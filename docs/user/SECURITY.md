<div align="center">

# Security policy

What the proxy is allowed to do, and what it refuses.

</div>

---

## What this project does

`opengl32.dll` is loaded into the game process and patches memory in that process.
It can also load native DLLs from the `coke/` folder when a matching
`user_consent_<stem>.txt` file is present. A loaded DLL runs with the same
privileges as the game; the consent file is an acknowledgement, not a sandbox.

## Build profiles are trusted input

`profiles/<exe>/<build>.ini` decides which engine addresses the proxy may write.
A profile is therefore trusted input, on the same level as the DLL itself: only
install profiles you wrote or obtained from this repository.

The proxy reduces the blast radius by refusing anything inconsistent:

- addresses are module-relative and must fall inside the live image;
- slot addresses are only used while their content still points at the real
  user32 function;
- the engine app object is only adopted when it carries this process's HWND and
  a canvas the engine has actually used;
- every write re-validates its target immediately before writing.

A malicious profile can still make the game crash or corrupt its own state. It
cannot load a DLL, spawn a process or execute code outside the game process.

## Reporting a vulnerability

Please **do not** open a public issue for a security problem. Report it privately
to aktionkokain@gmail.com, or open a minimal issue that
asks for a private contact channel.

Include:

- affected version/commit;
- Windows version and game build;
- steps to reproduce;
- impact (code execution, privilege issue, unsafe write, etc.).

## Scope

In scope:

- memory-safety bugs in the proxy that can be triggered by game files, mod files
  or window messages;
- unsafe path/loader behaviour (`opengl32_real.dll`, `coke/*.dll`);
- a crash that corrupts the game or the system rather than just the process.

Out of scope:

- the game's own vulnerabilities;
- a malicious mod that the user explicitly consented to load;
- anti-cheat or DRM concerns in a single-player game.

## Verifying releases

Build from source when possible. If you download a release, check that it comes
from this repository and compare the SHA-256 published with the release. Never
run a DLL from an unknown source.
