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
then the current directory and `PATH`. The renderer's D3D12 backend compiles
shaders with FXC (`d3dcompiler_47.dll`, which ships with Windows) rather than
wgpu's default, which first looks for `dxcompiler.dll`. Windows does not ship
that library, so the search could reach a copy planted in the current
directory. The process environment is trusted: `WGPU_DX12_COMPILER=dxc` or
`auto` still opts in to that search.

wgpu's Vulkan backend loads `vulkan-1.dll` the same way. GPU drivers install it
in the system directory, but on a machine without a Vulkan runtime the search
continues into the current directory. The renderer does not change
process-wide DLL search settings. An application that opens user-chosen or
downloaded folders should keep its current directory out of them, or call
`SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)` at startup, which
removes the current directory and `PATH` from the search, before it creates a
renderer.
