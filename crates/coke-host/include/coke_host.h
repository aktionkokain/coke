#ifndef COKE_HOST_H
#define COKE_HOST_H

#include <stddef.h>
#include <stdint.h>

#if !defined(_WIN32) || !(defined(_M_IX86) || defined(__i386__))
#error The Coke native mod ABI requires 32-bit Windows.
#endif

#define COKE_CALL __stdcall
#define COKE_HOST_API 8u
#define COKE_PROF_VERSION 2u

/* Match the x86 Windows ABI regardless of the consumer's ambient packing. */
#pragma pack(push, 8)

typedef struct CokeProfStats {
    uint32_t version;
    uint32_t size;
    uint64_t frames;
    uint64_t frame_ns;
    uint64_t last_frame_ns;
    uint64_t swap_calls;
    uint64_t swap_ns;
    uint64_t last_swap_ns;
    uint64_t call_list;
    uint64_t call_list_ns;
    uint64_t bind_texture;
    uint64_t bind_texture_ns;
    uint64_t begin_end;
    uint64_t push_pop;
    uint64_t draw_arrays;
    uint64_t draw_elements;
    uint64_t vertices;
    uint64_t tex_upload;
    uint64_t tex_upload_bytes;
    uint64_t read_pixels;
    uint64_t read_pixels_ns;
    uint64_t copy_tex;
    uint64_t lists_new;
    uint64_t lists_called;
    uint64_t lists_deleted;
    uint64_t cpu_frames;
    uint64_t cpu_frame_ns;
    uint64_t last_cpu_frame_ns;
} CokeProfStats;

typedef struct CokeCamera {
    float pos[3];
    float dir[3];
    float up[3];
    float fov;
    float aspect;
    float near;
    float far;
} CokeCamera;

typedef struct CokeCameraCommand {
    float pan_right;
    float pan_forward;
    float yaw;
    float pitch;
    float zoom;
} CokeCameraCommand;

typedef struct CokeTerrain {
    uint32_t map;
    int32_t grid_w;
    int32_t grid_h;
    float world_w;
    float world_h;
    uint32_t heights;
} CokeTerrain;

typedef struct WorldVertex {
    float x;
    float y;
    float z;
    uint32_t rgba;
} WorldVertex;

typedef struct CokeRangePoint {
    float x;
    float y;
    float z;
} CokeRangePoint;

typedef struct CokeRangeResult {
    uint32_t status;
    CokeRangePoint origin;
    CokeRangePoint target;
    float distance_m;
    float visible_fraction;
    int32_t cursor_x;
    int32_t cursor_y;
} CokeRangeResult;

typedef void (COKE_CALL *CokeHotkeyFn)(void);

typedef int32_t (COKE_CALL *LosScanFn)(float, float, float, float, float, float, uint8_t *, uint32_t, float *, uint32_t, int32_t *, int32_t *);

typedef int32_t (COKE_CALL *LosScanMapFn)(float, float, float, float, uint8_t *, uint32_t, float *, uint32_t, int32_t *, int32_t *);

typedef int32_t (COKE_CALL *RegisterHotkeyFn)(int32_t, CokeHotkeyFn);

typedef int32_t (COKE_CALL *CameraApplyFn)(const CokeCameraCommand *);

typedef float (COKE_CALL *CameraWheelFn)(void);

typedef int32_t (COKE_CALL *RangeControlFn)(uint32_t, const CokeRangePoint *);

typedef int32_t (COKE_CALL *RangeReadFn)(CokeRangeResult *);

typedef uint32_t (COKE_CALL *RangeCapabilitiesFn)(void);

typedef struct CokeHost {
    uint32_t api_version;
    void (COKE_CALL *log)(const char *);
    void (COKE_CALL *draw_text)(int32_t, int32_t, const char *, uint32_t);
    int32_t (COKE_CALL *get_width)(void);
    int32_t (COKE_CALL *get_height)(void);
    float (COKE_CALL *get_fps)(void);
    uint32_t size;
    uint32_t (COKE_CALL *read_u32)(uint32_t);
    float (COKE_CALL *read_f32)(uint32_t);
    int32_t (COKE_CALL *read_bytes)(uint32_t, uint8_t *, uint32_t);
    uint32_t (COKE_CALL *module_base)(void);
    uint32_t (COKE_CALL *module_size)(void);
    int32_t (COKE_CALL *class_name)(uint32_t, char *, uint32_t);
    int32_t (COKE_CALL *scene_rect)(int32_t *);
    int32_t (COKE_CALL *camera)(CokeCamera *);
    int32_t (COKE_CALL *world_to_screen)(float, float, float, float *, float *);
    int32_t (COKE_CALL *cursor_world)(float *, float *, float *);
    int32_t (COKE_CALL *terrain)(CokeTerrain *);
    float (COKE_CALL *terrain_height)(float, float);
    LosScanFn los_scan;
    int32_t (COKE_CALL *los_segment)(float, float, float, float, float, float);
    uint32_t (COKE_CALL *selected_unit)(void);
    int32_t (COKE_CALL *unit_pos)(uint32_t, float *, float *, float *);
    void (COKE_CALL *draw_world_triangles)(const WorldVertex *, uint32_t);
    int32_t (COKE_CALL *key_down)(int32_t);
    int32_t (COKE_CALL *focused)(void);
    void (COKE_CALL *debug_dump)(void);
    uint32_t (COKE_CALL *time_ms)(void);
    void (COKE_CALL *draw_world_lines)(const WorldVertex *, uint32_t, float);
    int32_t (COKE_CALL *cursor_ground)(float *, float *, float *);
    LosScanMapFn los_scan_map;
    RegisterHotkeyFn register_hotkey;
    void (COKE_CALL *set_layer)(int32_t);
    CameraApplyFn camera_apply;
    CameraWheelFn camera_wheel_delta;
    int32_t (COKE_CALL *camera_input_capture)(int32_t);
    RangeControlFn range_control;
    RangeReadFn range_read;
    RangeCapabilitiesFn range_capabilities;
} CokeHost;

#pragma pack(pop)

#define COKE_HOST_V1_SIZE   offsetof(CokeHost, size)
#define COKE_HOST_V2_SIZE   offsetof(CokeHost, draw_world_lines)
#define COKE_HOST_V2_1_SIZE offsetof(CokeHost, register_hotkey)
#define COKE_HOST_V4_SIZE   offsetof(CokeHost, camera_apply)
#define COKE_HOST_V6_SIZE   offsetof(CokeHost, range_control)
#define COKE_HOST_V7_SIZE   offsetof(CokeHost, range_capabilities)
#define COKE_HOST_V8_SIZE   sizeof(CokeHost)

#define COKE_RANGE_ENABLE_KEYS       1u
#define COKE_RANGE_FROM_CURSOR       2u
#define COKE_RANGE_SET_ORIGIN        3u
#define COKE_RANGE_CANCEL            4u
#define COKE_RANGE_SET_SOURCE_HEIGHT 5u
#define COKE_RANGE_CAP_PROFILED      1u
#define COKE_RANGE_CAP_INSTALLED     2u
#define COKE_RANGE_WAITING           1u
#define COKE_RANGE_CLEAR             2u
#define COKE_RANGE_BLOCKED           3u

/*
 * The host supplies CokeHost to coke_init. Check api_version and size before
 * reading appended fields. Return 0 to decline. Register hotkeys and choose a
 * draw layer synchronously during init. F9 and F11 are reserved.
 *
 * coke_frame and coke_hud run on the presentation thread. Only they may use
 * draw services. coke_tick and hotkey callbacks run on other threads. The
 * loader keeps accepted DLLs loaded for the process lifetime.
 *
 * World is measured in metres, with x east, y north and z up. Screen and
 * cursor positions are physical client pixels from the top left. Text takes
 * 0x00RRGGBB, and WorldVertex.rgba takes 0xAABBGGRR. Arrays use element counts.
 * Caller-owned outputs must be writable, aligned and disjoint. scene_rect
 * takes four int32_t values (x, y, width, height). Strings are NUL terminated.
 *
 * Look up coke_prof_version and coke_prof_read_sized on the proxy. Capacity is
 * in bytes. The legacy coke_prof_read requires the full frozen v2 snapshot.
 */
typedef int32_t (COKE_CALL *CokeInitFn)(const CokeHost *);
typedef void (COKE_CALL *CokeFrameFn)(int32_t, int32_t, float);
typedef void (COKE_CALL *CokeTickFn)(void);
typedef uint32_t (COKE_CALL *CokeProfVersionFn)(void);
typedef int32_t (COKE_CALL *CokeProfReadFn)(CokeProfStats *);
typedef int32_t (COKE_CALL *CokeProfReadSizedFn)(CokeProfStats *, uint32_t);

#endif
