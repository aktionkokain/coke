//! Engine widget-tree reader, dump and native re-layout.
//!
//! The engine builds its UI once: `FUN_007990a0` resolves every widget's
//! absolute rect against its parent at construction and stores it as four i16
//! at `+0x10..+0x17`. The persistent root widget lives at `app+0x34` and its
//! children vector is `{begin, end, capacity}` at `+0x3c / +0x40 / +0x44` -
//! note `+0x40` is the END pointer, not the begin: the old heap-scan code in
//! `widgets.rs` read `+0x40/+0x44` (end -> capacity) and therefore validated
//! garbage.
//!
//! `FUN_00799a30` / `FUN_00799bd0` draw a widget by calling
//! `glViewport(widget rect)` and building `glOrtho(0..w, 0..h)` from the same
//! rect, and `FUN_0054ee90` derives the 3D camera aspect from the live GL
//! viewport, so correcting the engine's rects is both necessary and sufficient
//! for drawing + picking.
//!
//! The dump (`dump_widget_tree`) is read-only. The re-layout
//! (`widget_relayout_tick`) snapshots the live tree after a canvas change,
//! replays the engine's layout rules against the new canvas, and writes the
//! new rects back. Rect sources confirmed from the binary:
//!
//! ```text
//! FUN_007a6b40  battle screen:
//!   screen  = ctor(parent, 0,   0,     -1,          -1)          // fill
//!   scene   = ctor(screen, 0,   0x9b,  canvas_w,    canvas_h-0x9b)
//!   flank   = (canvas_w - 0x400) / 2
//!   left    = ctor(screen, 0,   0,     flank,       0x9b)
//!   right   = ctor(screen, canvas_w-flank, 0, flank, 0x9b)
//!   bar     = ctor(screen, flank, 0,   0x400,       0xa4)
//! ```
//!
//! and menus/dialogs are `ctor(parent, -1, -1, 0x400, 0x300)` = centred.
//!
//! The re-layout pass is scheduled after a canvas change, then identifies
//! nodes by RTTI class and applies the rules above against the new canvas.

use crate::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};
use std::collections::{HashMap, HashSet};

const MAX_NODES: usize = 4096;
const MAX_DEPTH: u32 = 24;
const MAX_KIDS: usize = 2048;
const MAX_LOG_NODES: usize = 1500;
const MAX_DUMPS: u32 = 16;

/// Dump reasons, only used for the log tag.
pub const DUMP_ON_RESIZE: u32 = 1;
pub const DUMP_ON_KEY: u32 = 2;
pub const DUMP_ON_BATTLE: u32 = 3;

static DUMP_PENDING: AtomicU32 = AtomicU32::new(0);
static DUMP_REASON: AtomicU32 = AtomicU32::new(0);
static DUMP_NOT_BEFORE: AtomicU32 = AtomicU32::new(0);
static DUMP_SEQ: AtomicU32 = AtomicU32::new(0);
static BATTLE_DUMPED: AtomicU32 = AtomicU32::new(0);

/// One widget node as read from the live process.
#[derive(Clone, Copy)]
pub struct Node {
    pub addr: usize,
    pub parent: usize,
    pub depth: u32,
    pub rect: [i32; 4],
    pub vt: u32,
    /// RTTI class of the widget (`View`, `GameView`, `CommandView`, ...).
    pub class: WidgetClass,
    pub begin: usize,
    pub end: usize,
    pub cap: usize,
}

// ------------------------------------------------------------ fault-safe reads

#[inline]
unsafe fn rd(addr: usize, buf: &mut [u8]) -> bool {
    unsafe {
        if addr < 0x10000 {
            return false;
        }
        let mut got = 0usize;
        ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
            &mut got,
        ) != 0
            && got == buf.len()
    }
}

#[inline]
unsafe fn rd_u32(addr: usize) -> Option<u32> {
    unsafe {
        let mut b = [0u8; 4];
        if rd(addr, &mut b) {
            Some(u32::from_le_bytes(b))
        } else {
            None
        }
    }
}

/// Four i16 at `+0x10..+0x17`: absolute x, y, w, h (top-down y).
#[inline]
unsafe fn rd_rect(addr: usize) -> Option<[i32; 4]> {
    unsafe {
        let mut b = [0u8; 8];
        if !rd(addr, &mut b) {
            return None;
        }
        Some([
            i16::from_le_bytes([b[0], b[1]]) as i32,
            i16::from_le_bytes([b[2], b[3]]) as i32,
            i16::from_le_bytes([b[4], b[5]]) as i32,
            i16::from_le_bytes([b[6], b[7]]) as i32,
        ])
    }
}

#[inline]
fn in_image(v: u32, base: usize, end: usize) -> bool {
    (v as usize) >= base && (v as usize) < end
}

#[inline]
fn module_range() -> (usize, usize) {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 {
            return (0, 0);
        }
        let size = module_image_size(base);
        (base, base.saturating_add(size))
    }
}

// --------------------------------------------------------------- scheduling

/// Ask for one dump. Cheap enough to call from the GL or window thread; the
/// watcher thread performs the walk.
pub fn schedule_widget_dump(reason: u32) {
    if !config().widget_dump {
        return;
    }
    // A canvas change starts a new layout epoch: allow the next battle frame to
    // arm its own dump again.
    if reason == DUMP_ON_RESIZE {
        BATTLE_DUMPED.store(0, Ordering::Relaxed);
    }
    if reason == DUMP_ON_BATTLE && BATTLE_DUMPED.swap(1, Ordering::Relaxed) != 0 {
        return;
    }
    DUMP_REASON.store(reason, Ordering::Relaxed);
    // Let the engine finish the resize/build (and the re-layout) before reading
    // the tree.
    DUMP_NOT_BEFORE.store(unsafe { GetTickCount() } + 900, Ordering::Relaxed);
    DUMP_PENDING.store(1, Ordering::Relaxed);
}

fn reason_tag(reason: u32) -> &'static str {
    match reason {
        DUMP_ON_RESIZE => "resize",
        DUMP_ON_KEY => "F9",
        DUMP_ON_BATTLE => "battle",
        _ => "?",
    }
}

/// Called from the watcher loop every tick.
pub unsafe fn widget_dump_tick() {
    unsafe {
        if !config().widget_dump || DUMP_PENDING.load(Ordering::Relaxed) == 0 {
            return;
        }
        let now = GetTickCount();
        // NOT_BEFORE is in the future -> bit 31 set after the wrapping subtract.
        if now.wrapping_sub(DUMP_NOT_BEFORE.load(Ordering::Relaxed)) > 0x8000_0000 {
            return;
        }
        DUMP_PENDING.store(0, Ordering::Relaxed);
        let n = DUMP_SEQ.fetch_add(1, Ordering::Relaxed);
        if n >= MAX_DUMPS {
            if n == MAX_DUMPS {
                log("widget dump: limit reached; no further automatic dumps");
            }
            return;
        }
        let reason = DUMP_REASON.load(Ordering::Relaxed);
        dump_widget_tree(reason);
    }
}

// ------------------------------------------------------------------- walking

/// Walk the live tree breadth-first from the root widget. Read-only.
pub unsafe fn collect_tree() -> (usize, Vec<Node>) {
    unsafe {
        let (base, end) = module_range();
        if base == 0 || end <= base {
            return (0, Vec::new());
        }
        let Some(app) = app_object() else {
            return (0, Vec::new());
        };
        let Some(root) = rd_u32(app + 0x34) else {
            return (app, Vec::new());
        };
        let root = root as usize;

        let mut out: Vec<Node> = Vec::new();
        let mut seen: HashSet<usize> = HashSet::new();
        // (addr, parent, depth), popped LIFO; children pushed reversed so the
        // traversal keeps the engine's child order in the log.
        let mut stack: Vec<(usize, usize, u32)> = vec![(root, 0, 0)];

        while let Some((addr, parent, depth)) = stack.pop() {
            if out.len() >= MAX_NODES {
                break;
            }
            if !seen.insert(addr) {
                continue;
            }
            let Some(vt) = rd_u32(addr) else { continue };
            if !in_image(vt, base, end) {
                continue;
            }
            let Some(rect) = rd_rect(addr + 0x10) else {
                continue;
            };
            // Zero-sized containers are legitimate; out-of-range numbers are not
            // (the canvas itself is capped at 16384).
            if rect[2] < 0 || rect[3] < 0 || rect[2] > 0x4000 || rect[3] > 0x4000 {
                continue;
            }
            let begin = rd_u32(addr + 0x3c).unwrap_or(0) as usize;
            let endp = rd_u32(addr + 0x40).unwrap_or(0) as usize;
            let cap = rd_u32(addr + 0x44).unwrap_or(0) as usize;
            out.push(Node {
                addr,
                parent,
                depth,
                rect,
                vt,
                class: classify_vt(vt),
                begin,
                end: endp,
                cap,
            });

            if depth >= MAX_DEPTH {
                continue;
            }
            if begin == 0 || endp <= begin || (begin & 3) != 0 {
                continue;
            }
            let n = (endp - begin) / 4;
            if n == 0 || n > MAX_KIDS {
                continue;
            }
            let mut kids: Vec<usize> = Vec::with_capacity(n);
            for i in 0..n {
                if let Some(k) = rd_u32(begin + i * 4) {
                    let k = k as usize;
                    if k > 0x10000 {
                        kids.push(k);
                    }
                }
            }
            for k in kids.into_iter().rev() {
                stack.push((k, addr, depth + 1));
            }
        }
        (app, out)
    }
}

// ------------------------------------------------------------ classification

/// Which edge of the parent the widget is pinned to, per axis.
/// Order: stretch > left > right > centre > offset.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Anchor {
    Stretch,
    Left,
    Right,
    Centre,
    Offset,
}

impl Anchor {
    fn tag(self) -> char {
        match self {
            Anchor::Stretch => 'S',
            Anchor::Left => 'L',
            Anchor::Right => 'R',
            Anchor::Centre => 'C',
            Anchor::Offset => 'O',
        }
    }
}

fn classify(rel: i32, size: i32, parent: i32) -> Anchor {
    if size == parent {
        Anchor::Stretch
    } else if rel == 0 {
        Anchor::Left
    } else if rel + size == parent || rel + size + 1 == parent {
        Anchor::Right
    } else if (2 * rel + size - parent).abs() <= 1 {
        Anchor::Centre
    } else {
        Anchor::Offset
    }
}

// --------------------------------------------------------------------- dump

/// Read-only dump of the current widget tree. Logs one line per node plus a
/// summary with per-axis anchor tallies and the scene/blur rects.
pub unsafe fn dump_widget_tree(reason: u32) {
    unsafe {
        let (app, nodes) = collect_tree();
        let (lw, lh) = logical_size();
        let tag = reason_tag(reason);
        if app == 0 {
            log(&format!("widget dump: tag={tag} no app object yet"));
            return;
        }
        let canvas = (
            rd_u32(app).unwrap_or(0) as i32,
            rd_u32(app + 4).unwrap_or(0) as i32,
        );
        let root = rd_u32(app + 0x34).unwrap_or(0) as usize;
        log(&format!(
            "widget dump: tag={tag} logical={lw}x{lh} app=0x{app:x} app_canvas={}x{} \
         root=0x{root:x} nodes={}",
            canvas.0,
            canvas.1,
            nodes.len()
        ));
        if nodes.is_empty() {
            log("widget dump: empty tree (app or root not ready / unsupported build)");
            return;
        }

        // Index parents so relative geometry and orphan detection are cheap.
        let mut by_addr: Vec<(usize, [i32; 4])> = nodes.iter().map(|n| (n.addr, n.rect)).collect();
        by_addr.sort_unstable_by_key(|e| e.0);
        let lookup = |a: usize| -> Option<[i32; 4]> {
            by_addr
                .binary_search_by_key(&a, |e| e.0)
                .ok()
                .map(|i| by_addr[i].1)
        };

        let mut max_depth = 0u32;
        let mut orphans = 0usize;
        let mut tallies_x = [0usize; 5];
        let mut tallies_y = [0usize; 5];
        let mut logged = 0usize;
        let scene_addr = scene_widget();

        for n in nodes.iter() {
            max_depth = max_depth.max(n.depth);
            let (px, py, pw, ph) = if n.parent == 0 {
                (0, 0, lw.max(1), lh.max(1))
            } else {
                match lookup(n.parent) {
                    Some(p) => (p[0], p[1], p[2], p[3]),
                    None => {
                        orphans += 1;
                        (0, 0, lw.max(1), lh.max(1))
                    }
                }
            };
            let rel_x = n.rect[0] - px;
            let rel_y = n.rect[1] - py;
            let ax = classify(rel_x, n.rect[2], pw.max(1));
            let ay = classify(rel_y, n.rect[3], ph.max(1));
            if n.parent != 0 {
                tallies_x[ax as usize] += 1;
                tallies_y[ay as usize] += 1;
            }
            if logged >= MAX_LOG_NODES {
                continue;
            }
            logged += 1;
            let kids = if n.end > n.begin {
                (n.end - n.begin) / 4
            } else {
                0
            };
            let tail = if n.end > n.begin && n.cap > n.end {
                (n.cap - n.end) / 4
            } else {
                0
            };
            let mark = if n.addr == root {
                " [root]"
            } else if Some(n.addr) == scene_addr {
                " [scene]"
            } else {
                ""
            };
            log(&format!(
                "widget tree: d={} a=0x{:x} vt=0x{:08x} cls={} p=0x{:x} rect=({},{},{},{}) \
             rel=({},{}) anc={}/{} kids=0x{:x}/0x{:x}/0x{:x} n={} oldbug_reads={}{}",
                n.depth,
                n.addr,
                n.vt,
                n.class.tag(),
                n.parent,
                n.rect[0],
                n.rect[1],
                n.rect[2],
                n.rect[3],
                rel_x,
                rel_y,
                ax.tag(),
                ay.tag(),
                n.begin,
                n.end,
                n.cap,
                kids,
                tail,
                mark
            ));
        }

        let scene = scene_widget();
        let blur = blur_quad();
        log(&format!(
            "widget dump: summary nodes={} maxdepth={} orphans={} logged={} \
         anchors_x[S{} L{} R{} C{} O{}] anchors_y[S{} L{} R{} C{} O{}] \
         scene=0x{:x} scene_rect={:?} blur=0x{:x} blur_rect={:?}",
            nodes.len(),
            max_depth,
            orphans,
            logged,
            tallies_x[0],
            tallies_x[1],
            tallies_x[2],
            tallies_x[3],
            tallies_x[4],
            tallies_y[0],
            tallies_y[1],
            tallies_y[2],
            tallies_y[3],
            tallies_y[4],
            scene.unwrap_or(0),
            scene.map(|s| rd_rect(s + 0x10).unwrap_or([0, 0, 0, 0])),
            blur.unwrap_or(0),
            blur.map(|b| read_blur_rect(b).unwrap_or([0, 0, 0, 0]))
        ));
        if let Some(s) = scene
            && let Some(sc) = profile::scene()
            && let Some(owner) = profile_addr(sc.owner_ptr).and_then(|g| rd_u32(g))
        {
            log(&format!(
                "widget dump: scene owner=0x{:x} widget=0x{:x} rect={:?}",
                owner,
                s,
                rd_rect(s + sc.widget_rect_off as usize).unwrap_or([0, 0, 0, 0])
            ));
        }
        if let Some(b) = blur {
            let sc = profile::scene();
            log(&format!(
                "widget dump: blur quad=0x{:x} rect={:?} enabled={} backlink=0x{:x}",
                b,
                read_blur_rect(b).unwrap_or([0, 0, 0, 0]),
                rd_u32(b + sc.map(|s| s.blur_quad_enable_off as usize).unwrap_or(0x50))
                    .unwrap_or(0),
                rd_u32(
                    b + sc
                        .map(|s| s.blur_quad_backlink_off as usize)
                        .unwrap_or(0x54)
                )
                .unwrap_or(0)
            ));
        }
    }
}

/// The widget that owns the 3D scene.
///
/// Preferred anchor: the profile's stable scene-owner global. Some builds
/// (Cold War) only keep the battle screen in the heap, reachable through the
/// widget tree. In that case `[scene] widget_class` names the RTTI class to
/// resolve from the live tree instead (e.g. `GameView`).
pub unsafe fn scene_widget() -> Option<usize> {
    unsafe {
        let sc = profile::scene()?;
        if sc.owner_ptr != 0 {
            let owner = rd_u32(profile_addr(sc.owner_ptr)?)? as usize;
            if owner < 0x10000 {
                return None;
            }
            return rd_u32(owner + sc.owner_widget_off as usize)
                .map(|v| v as usize)
                .filter(|v| *v > 0x10000);
        }
        let class = sc.widget_class.as_deref()?;
        let (_app, nodes) = collect_tree();
        nodes
            .iter()
            .find(|n| n.depth > 0 && n.class.tag().eq_ignore_ascii_case(class))
            .map(|n| n.addr)
    }
}

/// The profile's blur-quad global, created when the battle scene is built.
unsafe fn blur_quad() -> Option<usize> {
    unsafe {
        rd_u32(profile_addr(profile::scene()?.blur_quad_ptr)?)
            .map(|v| v as usize)
            .filter(|v| *v > 0x10000)
    }
}

unsafe fn read_blur_rect(b: usize) -> Option<[i32; 4]> {
    unsafe {
        let off = profile::scene()?.blur_quad_rect_off as usize;
        Some([
            rd_u32(b + off)? as i32,
            rd_u32(b + off + 4)? as i32,
            rd_u32(b + off + 8)? as i32,
            rd_u32(b + off + 12)? as i32,
        ])
    }
}

// ------------------------------------------------------------ native re-layout

static RELAYOUT_PENDING: AtomicU32 = AtomicU32::new(0);
static RELAYOUT_NOT_BEFORE: AtomicU32 = AtomicU32::new(0);
static RELAYOUT_T0: AtomicU32 = AtomicU32::new(0);
static RELAYOUT_SEQ: AtomicU32 = AtomicU32::new(0);

// Axis anchor codes (also used in the log).
const AX_NONE: u8 = 0;
const AX_STRETCH: u8 = 1;
const AX_LEFT: u8 = 2;
const AX_RIGHT: u8 = 3;
const AX_CENTRE: u8 = 4;
const AX_EXTEND: u8 = 5;

fn axis_tag(a: u8) -> char {
    match a {
        AX_STRETCH => 'S',
        AX_LEFT => 'L',
        AX_RIGHT => 'R',
        AX_CENTRE => 'C',
        AX_EXTEND => 'E',
        _ => 'O',
    }
}

/// One planned rect change. `why` is empty for a generic anchor replay and set
/// for the explicit battle-screen rules.
#[derive(Clone, Copy)]
pub struct NewRect {
    pub addr: usize,
    pub parent: usize,
    pub old: [i32; 4],
    pub new: [i32; 4],
    pub why: &'static str,
    pub ax: u8,
    pub ay: u8,
}

/// One axis: keep the relative offset, or re-anchor to the new parent size.
/// `vertical` enables the "top inset + extend to the bottom" rule used by the
/// battle scene (`y = 0x9b, h = canvas_h - 0x9b`).
fn axis_plan(rel: i32, size: i32, p_old: i32, p_new: i32, vertical: bool) -> (i32, i32, u8) {
    let p_old = p_old.max(1);
    if size == p_old {
        return (0, p_new.max(1), AX_STRETCH);
    }
    if rel == 0 {
        return (0, size, AX_LEFT);
    }
    if rel + size == p_old || rel + size + 1 == p_old {
        if vertical && rel > 0 && size * 2 >= p_old {
            return (rel, (p_new - rel).max(1), AX_EXTEND);
        }
        return ((p_new - size).max(0), size, AX_RIGHT);
    }
    if (2 * rel + size - p_old).abs() <= 1 {
        // Signed: a widget wider than its parent stays mathematically centred
        // (clipped on both sides) instead of being pinned to x = 0, which would
        // erase the centring signal for every future pass.
        return ((p_new - size) / 2, size, AX_CENTRE);
    }
    (rel, size, AX_NONE)
}

/// Pure layout planner: parents before children, so each node re-anchors
/// against the parent's *new* rect while using the parent's *old* rect (from
/// the snapshot) to infer the anchor.
pub fn plan_layout(nodes: &[Node], scene: Option<usize>, new_w: i32, new_h: i32) -> Vec<NewRect> {
    let mut idx: HashMap<usize, usize> = HashMap::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        idx.insert(n.addr, i);
    }
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|&i| nodes[i].depth);

    let mut by_addr: HashMap<usize, [i32; 4]> = HashMap::with_capacity(nodes.len());
    let mut out: Vec<NewRect> = Vec::with_capacity(nodes.len());
    for i in order {
        let n = &nodes[i];
        let (new, why, ax, ay) = if n.parent == 0 {
            (
                [0, 0, new_w.max(1), new_h.max(1)],
                "root",
                AX_STRETCH,
                AX_STRETCH,
            )
        } else if let (Some(&pi), Some(pnew)) =
            (idx.get(&n.parent), by_addr.get(&n.parent).copied())
        {
            let pold = nodes[pi].rect;
            let rel_x = n.rect[0] - pold[0];
            let rel_y = n.rect[1] - pold[1];
            let (nx, nw, ax) = axis_plan(rel_x, n.rect[2], pold[2], pnew[2], false);
            let (ny, nh, ay) = axis_plan(rel_y, n.rect[3], pold[3], pnew[3], true);
            ([pnew[0] + nx, pnew[1] + ny, nw, nh], "", ax, ay)
        } else {
            (n.rect, "orphan", AX_NONE, AX_NONE)
        };
        by_addr.insert(n.addr, new);
        out.push(NewRect {
            addr: n.addr,
            parent: n.parent,
            old: n.rect,
            new,
            why,
            ax,
            ay,
        });
    }
    if let Some(scene) = scene {
        battle_override(nodes, &idx, &mut out, scene);
    }
    out
}

/// `FUN_007a6b40` lays the battle bar out with explicit canvas formulas:
/// `flank = (canvas_w - 0x400) / 2`, scene `(0, 0x9b, canvas_w, canvas_h-0x9b)`,
/// left `(0,0,flank,0x9b)`, right `(canvas_w-flank,0,flank,0x9b)`,
/// bar `(flank,0,0x400,0xa4)`. The generic anchors cannot see the flank width
/// (it is `(W-1024)/2`, not a fixed size), so apply the exact rule.
///
/// Roles are taken from the RTTI class, never from the current geometry:
/// the bar is the battle-screen child that owns the `CommandView`/`UnitView`,
/// the scene is the `GameView`, and the flanks are the remaining `View`s.
/// That keeps the pass idempotent: a bar clamped to x=0 by an earlier narrow
/// canvas is still recognised and re-centred when the canvas grows.
///
/// The bar's x is **signed** (`(W - 1024) / 2`), so it stays mathematically
/// centred below 1024 px and is simply clipped symmetrically. Only the flank
/// *widths* are clamped to zero.
fn battle_override(nodes: &[Node], idx: &HashMap<usize, usize>, out: &mut [NewRect], scene: usize) {
    const BAR_W: i32 = 0x400;
    const BAR_H: i32 = 0xa4;
    const FLANK_H: i32 = 0x9b;

    let Some(&si) = idx.get(&scene) else { return };
    let screen = nodes[si].parent;
    if screen == 0 {
        return;
    }
    let Some(&sci) = idx.get(&screen) else { return };
    let sold = nodes[sci].rect;
    let Some(snew) = out.iter().find(|p| p.addr == screen).map(|p| p.new) else {
        return;
    };
    if snew[2] <= 0 || snew[3] <= 0 {
        return;
    }
    let kids: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.parent == screen && n.addr != scene)
        .collect();

    // The bar is the screen child whose subtree owns the command/unit panel.
    let mut bar_addr = 0usize;
    'roles: for class in [WidgetClass::CommandView, WidgetClass::UnitView] {
        // There can be several CommandViews in a battle (floating menus), so
        // only accept one whose parent is a direct child of the battle screen
        // *and* also owns the sibling UnitView/SoldierView of the HUD bar.
        for n in nodes.iter().filter(|n| n.class == class) {
            if !kids.iter().any(|k| k.addr == n.parent) {
                continue;
            }
            let has_hud_sibling = nodes.iter().any(|c| {
                c.parent == n.parent
                    && matches!(c.class, WidgetClass::UnitView | WidgetClass::SoldierView)
            });
            // A bar with only a CommandView (no unit/soldier panel) is still
            // accepted when it is the sole container below the screen.
            let sole_container = kids
                .iter()
                .filter(|k| nodes.iter().any(|c| c.parent == k.addr))
                .count()
                == 1;
            if has_hud_sibling || sole_container {
                bar_addr = n.parent;
                break 'roles;
            }
        }
    }
    if bar_addr == 0 {
        // Structural fallback: the screen child that has children of its own.
        // (The bar is the only container among the screen's direct children.)
        // Deliberately class-independent: this still works if RTTI could not be
        // resolved yet (the packed image appears late in startup).
        if let Some(n) = kids
            .iter()
            .find(|n| nodes.iter().any(|c| c.parent == n.addr))
        {
            bar_addr = n.addr;
        }
    }
    let Some(bar) = kids.iter().find(|n| n.addr == bar_addr).copied() else {
        return;
    };

    let bar_h = if bar.rect[3] > 0 { bar.rect[3] } else { BAR_H };
    // Signed centre: never clamp this to 0 - the clamp is what used to destroy
    // the "this widget is centred" signal and pinned the HUD to the corner.
    let flank = (snew[2] - BAR_W) / 2;
    let flank_w = flank.max(0);

    // Scene: keep the top inset (0x9b), stretch to the new canvas.
    let sd = &nodes[si];
    let inset = if sd.rect[1] > 0 {
        sd.rect[1]
    } else if sd.rect[3] >= sold[3] - 1 {
        0
    } else {
        let c = config().scene_inset;
        if c > 0 { c } else { scene_inset() }
    };
    let inset = inset.clamp(0, (snew[3] - 1).max(0));
    set_new(
        out,
        scene,
        [snew[0], snew[1] + inset, snew[2], (snew[3] - inset).max(1)],
        "scene",
        AX_STRETCH,
        AX_EXTEND,
    );
    set_new(
        out,
        bar.addr,
        [snew[0] + flank, snew[1], BAR_W, bar_h],
        "bar",
        AX_NONE,
        AX_LEFT,
    );

    let flanks: Vec<&&Node> = kids
        .iter()
        .filter(|n| n.addr != bar.addr && n.class == WidgetClass::View)
        .collect();
    // Prefer the engine's own edges; fall back to order if a flank was already
    // collapsed to a zero-width rect at the wrong edge.
    let left = flanks
        .iter()
        .find(|n| n.rect[0] == sold[0])
        .or_else(|| flanks.first());
    let right = flanks
        .iter()
        .find(|n| n.rect[0] + n.rect[2] == sold[0] + sold[2])
        .or_else(|| flanks.last());
    if let Some(left) = left {
        set_new(
            out,
            left.addr,
            [
                snew[0],
                snew[1],
                flank_w,
                if left.rect[3] > 0 {
                    left.rect[3]
                } else {
                    FLANK_H
                },
            ],
            "flank-left",
            AX_STRETCH,
            AX_LEFT,
        );
    }
    if let Some(right) = right
        && right.addr != left.map(|l| l.addr).unwrap_or(0)
    {
        set_new(
            out,
            right.addr,
            [
                snew[0] + snew[2] - flank_w,
                snew[1],
                flank_w,
                if right.rect[3] > 0 {
                    right.rect[3]
                } else {
                    FLANK_H
                },
            ],
            "flank-right",
            AX_STRETCH,
            AX_LEFT,
        );
    }
}

fn set_new(out: &mut [NewRect], addr: usize, new: [i32; 4], why: &'static str, ax: u8, ay: u8) {
    if let Some(p) = out.iter_mut().find(|p| p.addr == addr) {
        p.new = new;
        p.why = why;
        p.ax = ax;
        p.ay = ay;
    }
}

static BATTLE_LAYOUT_DONE: AtomicU32 = AtomicU32::new(0);

/// Called on the first battle-scene frame: normalise a screen that was built
/// for a canvas narrower than the HUD (the engine's own ctor then gets negative
/// flank widths, which mean "inherit parent"). One pass per battle canvas.
pub fn schedule_battle_relayout() {
    if config().widget_relayout == 0 {
        return;
    }
    if BATTLE_LAYOUT_DONE.swap(1, Ordering::Relaxed) != 0 {
        return;
    }
    // `schedule_widget_relayout` clears the epoch flag as part of arming; set
    // it again so subsequent scene frames do not re-arm every 30 ms.
    schedule_widget_relayout();
    BATTLE_LAYOUT_DONE.store(1, Ordering::Relaxed);
}

/// Called from `patch_engine_canvas` after the engine canvas changed.
pub fn schedule_widget_relayout() {
    if config().widget_relayout == 0 {
        return;
    }
    // A new canvas starts a new epoch; the first battle frame may normalise
    // once more even if the battle screen was already visited.
    BATTLE_LAYOUT_DONE.store(0, Ordering::Relaxed);
    let now = unsafe { GetTickCount() };
    // Keep the first request's timestamp so the log shows the true latency
    // from "user resized" to "layout applied".
    if RELAYOUT_PENDING.swap(1, Ordering::Relaxed) == 0 {
        RELAYOUT_T0.store(now, Ordering::Relaxed);
    }
    let delay = config().relayout_delay_ms.min(2000);
    RELAYOUT_NOT_BEFORE.store(now.wrapping_add(delay), Ordering::Relaxed);
}

/// True while a re-layout is armed (the watcher polls fast then, so the delay
/// is not rounded up to the 50 ms idle tick).
pub fn relayout_pending() -> bool {
    RELAYOUT_PENDING.load(Ordering::Relaxed) != 0
}

/// Called from the watcher loop every tick.
pub unsafe fn widget_relayout_tick() {
    unsafe {
        let mode = config().widget_relayout;
        if mode == 0 || RELAYOUT_PENDING.load(Ordering::Relaxed) == 0 {
            return;
        }
        let now = GetTickCount();
        if now.wrapping_sub(RELAYOUT_NOT_BEFORE.load(Ordering::Relaxed)) > 0x8000_0000 {
            return;
        }
        RELAYOUT_PENDING.store(0, Ordering::Relaxed);
        let latency = now.wrapping_sub(RELAYOUT_T0.load(Ordering::Relaxed));

        let (app, nodes) = collect_tree();
        let (lw, lh) = logical_size();
        if app == 0 || nodes.is_empty() || lw < 320 || lh < 240 {
            return;
        }
        let scene = scene_widget();
        let plan = plan_layout(&nodes, scene, lw, lh);
        let dry = mode == 2;
        let changed = plan.iter().filter(|p| p.new != p.old).count();
        let (moved, skipped) = if dry {
            (0usize, 0usize)
        } else {
            apply_plan(&plan)
        };
        if RELAYOUT_SEQ.fetch_add(1, Ordering::Relaxed) < 40 {
            log(&format!(
                "widget relayout: {} logical={}x{} nodes={} changed={} moved={} skipped={} latency={}ms scene=0x{:x}",
                if dry { "dry-run" } else { "apply" },
                lw,
                lh,
                nodes.len(),
                changed,
                moved,
                skipped,
                latency,
                scene.unwrap_or(0)
            ));
            let mut n = 0usize;
            for p in plan.iter().filter(|p| p.new != p.old) {
                if n >= 160 {
                    break;
                }
                n += 1;
                log(&format!(
                    "widget relayout: {}0x{:x} ({},{},{},{}) -> ({},{},{},{}) {} axis={}/{}",
                    if dry { "dry " } else { "" },
                    p.addr,
                    p.old[0],
                    p.old[1],
                    p.old[2],
                    p.old[3],
                    p.new[0],
                    p.new[1],
                    p.new[2],
                    p.new[3],
                    if p.why.is_empty() { "-" } else { p.why },
                    axis_tag(p.ax),
                    axis_tag(p.ay)
                ));
            }
        }
        if !dry {
            // Read the tree back once the frame has settled, so the log shows the
            // post-relayout state next to the frame viewports.
            schedule_widget_dump(DUMP_ON_RESIZE);
        }
    }
}

unsafe fn apply_plan(plan: &[NewRect]) -> (usize, usize) {
    unsafe {
        let (base, end) = module_range();
        let mut moved = 0usize;
        let mut skipped = 0usize;
        for p in plan {
            if p.new == p.old {
                continue;
            }
            if write_rect(p, base, end) {
                moved += 1;
            } else {
                skipped += 1;
            }
        }
        (moved, skipped)
    }
}

/// Validate the node against the snapshot and write the 8-byte rect.
/// The rect is 8-byte aligned, so this is a single atomic store on x86.
unsafe fn write_rect(p: &NewRect, base: usize, end: usize) -> bool {
    unsafe {
        // The rect is four i16s. A negative width/height would be reinterpreted as
        // a huge unsigned value by the engine, so a broken plan must never be
        // written. Negative x/y is legitimate (centred content wider than its
        // parent); negative size is not.
        if p.new[2] < 0 || p.new[3] < 0 || p.new[2] > 0x4000 || p.new[3] > 0x4000 {
            return false;
        }
        let Some(vt) = rd_u32(p.addr) else {
            return false;
        };
        if !in_image(vt, base, end) {
            return false;
        }
        if p.parent != 0 {
            let Some(par) = rd_u32(p.addr + 4) else {
                return false;
            };
            if par as usize != p.parent {
                return false;
            }
        }
        // Nobody moved it since the snapshot (a stale plan must not win).
        if rd_rect(p.addr + 0x10) != Some(p.old) {
            return false;
        }
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&(p.new[0] as i16).to_le_bytes());
        b[2..4].copy_from_slice(&(p.new[1] as i16).to_le_bytes());
        b[4..6].copy_from_slice(&(p.new[2] as u16).to_le_bytes());
        b[6..8].copy_from_slice(&(p.new[3] as u16).to_le_bytes());
        let packed = u64::from_le_bytes(b);
        let target = (p.addr + 0x10) as *mut c_void;
        let mut old_prot = 0u32;
        if VirtualProtect(target, 8, PAGE_READWRITE, &mut old_prot) == 0 {
            return false;
        }
        core::ptr::write_unaligned((p.addr + 0x10) as *mut u64, packed);
        let mut ignored = 0u32;
        VirtualProtect(target, 8, old_prot, &mut ignored);
        rd_rect(p.addr + 0x10).map(|r| r == p.new).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nc(addr: usize, parent: usize, depth: u32, rect: [i32; 4], class: WidgetClass) -> Node {
        Node {
            addr,
            parent,
            depth,
            rect,
            vt: 0,
            class,
            begin: 0,
            end: 0,
            cap: 0,
        }
    }

    fn n(addr: usize, parent: usize, depth: u32, rect: [i32; 4]) -> Node {
        nc(addr, parent, depth, rect, WidgetClass::View)
    }

    fn get(p: &[NewRect], addr: usize) -> [i32; 4] {
        p.iter().find(|x| x.addr == addr).unwrap().new
    }

    fn battle_nodes(sw: i32, sh: i32, bar_x: i32, flank_w: i32) -> Vec<Node> {
        vec![
            nc(0x1000, 0, 0, [0, 0, sw, sh], WidgetClass::View),
            nc(0x1100, 0x1000, 1, [0, 0, sw, sh], WidgetClass::View),
            nc(
                0x1200,
                0x1100,
                2,
                [0, 155, sw, sh - 155],
                WidgetClass::GameView,
            ),
            nc(0x1300, 0x1100, 2, [0, 0, flank_w, 155], WidgetClass::View),
            nc(
                0x1400,
                0x1100,
                2,
                [sw - flank_w, 0, flank_w, 155],
                WidgetClass::View,
            ),
            nc(0x1500, 0x1100, 2, [bar_x, 0, 1024, 164], WidgetClass::View),
            nc(
                0x1600,
                0x1500,
                3,
                [bar_x, 0, 320, 164],
                WidgetClass::CommandView,
            ),
        ]
    }

    #[test]
    fn battle_screen_1920_to_2560() {
        // Exact node set observed in an F9 dump at 1920x1080.
        let nodes = battle_nodes(1920, 1080, 448, 448);
        let p = plan_layout(&nodes, Some(0x1200), 2560, 1439);
        assert_eq!(get(&p, 0x1000), [0, 0, 2560, 1439]);
        assert_eq!(get(&p, 0x1100), [0, 0, 2560, 1439]);
        assert_eq!(get(&p, 0x1200), [0, 155, 2560, 1284]);
        assert_eq!(get(&p, 0x1300), [0, 0, 768, 155]);
        assert_eq!(get(&p, 0x1400), [1792, 0, 768, 155]);
        assert_eq!(get(&p, 0x1500), [768, 0, 1024, 164]);
        assert_eq!(get(&p, 0x1600), [768, 0, 320, 164]);
    }

    #[test]
    fn menu_1920_to_2560() {
        let nodes = vec![
            n(0x2000, 0, 0, [0, 0, 1920, 1080]),
            n(0x2100, 0x2000, 1, [0, 0, 1920, 1080]),
            n(0x2200, 0x2100, 2, [448, 156, 1024, 768]), // centred panel
            n(0x2300, 0x2200, 3, [820, 581, 280, 26]),   // centred button
        ];
        let p = plan_layout(&nodes, None, 2560, 1377);
        assert_eq!(get(&p, 0x2100), [0, 0, 2560, 1377]);
        assert_eq!(get(&p, 0x2200), [768, 304, 1024, 768]);
        assert_eq!(get(&p, 0x2300), [1140, 729, 280, 26]);
    }

    #[test]
    fn shrink_back_to_1920() {
        let nodes = battle_nodes(2560, 1439, 768, 768);
        let p = plan_layout(&nodes, Some(0x1200), 1920, 1080);
        assert_eq!(get(&p, 0x1200), [0, 155, 1920, 925]);
        assert_eq!(get(&p, 0x1300), [0, 0, 448, 155]);
        assert_eq!(get(&p, 0x1400), [1472, 0, 448, 155]);
        assert_eq!(get(&p, 0x1500), [448, 0, 1024, 164]);
    }

    /// A narrow canvas must not pin the bar to
    /// x=0. The class roles must recognise it and put it back in the centre.
    #[test]
    fn degenerate_bar_recentres() {
        // Canvas is 2560 but the bar still sits at the left edge.
        let nodes = battle_nodes(2560, 1377, 0, 0);
        let p = plan_layout(&nodes, Some(0x1200), 2560, 1377);
        assert_eq!(get(&p, 0x1500), [768, 0, 1024, 164]);
        assert_eq!(get(&p, 0x1300), [0, 0, 768, 155]);
        assert_eq!(get(&p, 0x1400), [1792, 0, 768, 155]);
    }

    /// Below 1024 px the bar must stay mathematically centred (negative x,
    /// clipped) rather than be clamped to the left edge, which would lose the
    /// centring signal again.
    #[test]
    fn narrow_canvas_centres_bar_with_negative_x() {
        let nodes = battle_nodes(1254, 1080, 115, 115);
        let p = plan_layout(&nodes, Some(0x1200), 904, 1080);
        assert_eq!(get(&p, 0x1500), [-60, 0, 1024, 164]);
        assert_eq!(get(&p, 0x1300), [0, 0, 0, 155]);
        assert_eq!(get(&p, 0x1400), [904, 0, 0, 155]);
        // ... and growing back out of it recovers the centre immediately.
        let nodes = battle_nodes(904, 1080, -60, 0);
        let p = plan_layout(&nodes, Some(0x1200), 2560, 1377);
        assert_eq!(get(&p, 0x1500), [768, 0, 1024, 164]);
        assert_eq!(get(&p, 0x1300), [0, 0, 768, 155]);
    }
}
