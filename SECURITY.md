# Security policy

Report vulnerabilities privately through GitHub's security advisory interface.
Do not include private content, credentials, or player data in a report.

Scenes and resources are untrusted. Identity, revision, digest, dimensions,
counts, scalar values, and total pixels are validated before allocation or GPU
submission. Resource access is capability-based and bounded; renderer libraries
perform no ambient filesystem or network access. Output never overwrites an
existing path. Unsafe code is forbidden except for the isolated, documented SDL
raw-handle adapter required by wgpu.

## Native libraries on Windows

wgpu and SDL load several Windows libraries by bare name, so the standard DLL
search order applies: the application's own directory, the system directories,
then the current directory and `PATH`. The renderer keeps the wgpu loads it
selects from reaching the current directory:

- Direct3D 12 compiles shaders with FXC (`d3dcompiler_47.dll`, which ships
  with Windows) rather than wgpu's default, which first looks for
  `dxcompiler.dll`. Windows does not ship that library, so the search could
  reach a copy planted in the current directory. The process environment is
  trusted: `WGPU_DX12_COMPILER=dxc` or `auto` still opts in to that search.
- Vulkan is enabled only when `%SystemRoot%\System32\vulkan-1.dll` exists,
  which is where every Vulkan runtime installs its loader, so the search for
  it stops in the system directory. Without it, `Automatic` uses Direct3D 12
  alone and an explicit `Vulkan` request fails with `BackendUnavailable`.
  `Direct3D12` never loads the Vulkan loader. The check proves only that the
  file exists: if that copy cannot load, for example because it is built for
  another architecture, Windows may continue the search.

The renderer does not change process-wide DLL search settings, and libraries
it does not select can still search the current directory. SDL loads optional
libraries for the subsystems a consumer starts. With wgpu's validation flags
(on by default in debug builds, or set by `WGPU_VALIDATION` or
`WGPU_GPU_BASED_VALIDATION`), Microsoft's runtime loads its debug layers, such
as `D3D12SDKLayers.dll`, whose search order has not been verified. An
application that opens user-chosen or downloaded folders should keep its
current directory out of them, or call
`SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)` at startup, which
removes the current directory and `PATH` from the search, before it creates a
renderer.
