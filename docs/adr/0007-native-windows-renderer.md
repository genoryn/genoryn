# ADR 0007: native Windows Direct3D renderer

The actual Windows runtime smoke test on a Windows Server 2022 runner failed
with `egui_glow requires opengl 2.0+`. Compilation and headless UI-state tests
had passed; that was insufficient to establish native runtime compatibility.

Use eframe's maintained WGPU renderer with Direct3D 12 on Windows, including
the platform's software adapter where available. Keep the already-tested Glow
renderer on Linux. This is a native rendering-backend choice only; wallet IPC,
private-key ownership, monochrome views and consensus are unchanged. No graphics
driver download, remote execution or unsigned update is introduced.

The Windows test must create and render the real window, close after a bounded
number of frames, and return a successful exit code. It is not replaced by a
headless state test. Capture stderr and fail on initialization error or timeout.
Normal Windows users need a supported Direct3D 12 adapter/driver; unsupported
hardware must fail visibly rather than simulate a working wallet.
