# Focused compatibility sources

The bundled SeamFix 2.1 compatibility nodes preserve the serialized interfaces
and focused behavior of the projects used by the tutorial workflow. They do
not vendor those projects' unrelated node suites.

## ComfyUI MaraScott Nodes — McBoaty v2

- Source: <https://github.com/davask/ComfyUI_MaraScott_Nodes>
- Tutorial-era source reviewed at commit
  `90f3f800833400a5579ddc4ce00116c626974840`.
- Files studied: `McBoaty.py`, `McBoaty_v2.py`, and the focused image-grid
  helpers.
- The compatibility port retains the nine-tile overlap topology, upscale,
  VAE, sampler, decode, and feathered reconstruction behavior.

MIT License

Copyright (c) 2024 david asquiedge

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

Attribution is required. The use of this software must be accompanied by
proper credit to the original author.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## WAS Node Suite — Image Resize

- Source: <https://github.com/WASasquatch/was-node-suite-comfyui>
- Tutorial-era source reviewed at commit
  `15840cbdd68fe7f7c323495fbe03f1082177c379`.
- Only the `Image Resize` tensor/PIL rescale behavior and its exact workflow
  widget contract are represented.

MIT License

Copyright (c) 2023 Jordan Thompson (WASasquatch)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## ComfyUI Art Venture — ColorCorrect

- Source: <https://github.com/sipherxyz/comfyui-art-venture>
- Tutorial-era source reviewed at commit
  `80d18c23aaf2d66b2766ef815992218fac6a3543`.
- The compatibility node retains all six original controls. Its HLS/HSV math
  is implemented directly with PyTorch so the custom-node pack does not add an
  OpenCV package dependency.

MIT License

Copyright (c) 2025 VIXION

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## ComfyUI Impact Pack — PreviewBridge interface study

- Source: <https://github.com/comfyorg/comfyui-impact-pack>
- Tutorial-era behavior reviewed at commit
  `8acae9fe862fef3aab59aaf828aaa8ac9859e05d`.
- No Impact Pack module is vendored. `SeamFixPreviewBridge` is a focused,
  independently written adapter to current ComfyUI's native Mask Editor save
  contract: preview an IMAGE, accept the clipspace PNG written back to the
  node's `image` widget, and expose its inverted alpha as a MASK.

## KJNodes

`GrowMaskWithBlur` remains an external workflow dependency and is not copied
into this repository: <https://github.com/kijai/ComfyUI-KJNodes>.
