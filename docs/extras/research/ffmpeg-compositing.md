# ffmpeg compositing cookbook for the facecam

> Research notes made for the facecam design on 2026-10-22: web research and read-only code reading, nothing was run. Every claim carries a source and a confidence tag. Where this dossier and the spec (`docs/superpowers/specs/2026-10-20-facecam-design.md`) disagree, the spec wins: it weighs all four dossiers and the code review together.

Status: research only, 2026-10-22. **Nothing in here was run.** Every filter graph and command line below is **untested** unless a line says otherwise. Option names and filter behaviour come from the FFmpeg `release/9.0` source tree (the branch the pinned 9.0.1 is cut from) and from the ffmpeg.org documentation. Where a number is my own estimate and not a published measurement, it says *estimate*.

Context read (read-only): `docs/superpowers/specs/2026-10-20-facecam-design.md`, `src-tauri/src/{muxer,edit,export,tools,encode,stems,commands,mft,watermark}.rs`, `scripts/fetch-ffmpeg.mjs`, `src/routes/Recording.tsx`.

---

## 0. Summary

1. **Build:** gyan.dev `ffmpeg-9.0.1-essentials_build`. It has CPU `overlay`, `alphamerge`, `geq`, `scale`, `crop`, `pad`, `hflip`, `boxblur`, `tpad`, `trim` and `setpts`. It has `overlay_cuda`, `scale_cuda`, `pad_cuda` and `hwupload_cuda` (CUDA via `--enable-cuda-llvm`) and `overlay_qsv`, `vpp_qsv` and `scale_qsv` (libvpl). It has **no** `overlay_vulkan`, `libplacebo` or `overlay_opencl`, because Vulkan and OpenCL ship only in the *full* build. **AMF has no overlay filter at all.** Fabi should confirm all of this on the real binary (commands in 1.5).
2. **Default pipeline (all vendors):** decode the clean game on the GPU (`-hwaccel d3d11va`), do the cam work on the CPU (crop, hflip, scale, mask by `alphamerge` with a pre-rendered PNG, plus `overlay`), then encode with the configured hardware encoder using the same CQ/QP mapping as `edit.rs`. The cam is small, so the overlay costs almost nothing. **The encoder sets the speed.**
3. **Time alignment works for free in `overlay`.** Before the cam's first frame the game passes through untouched (framesync `before=EXT_NULL`). With `eof_action=pass` the cam disappears after its last frame. During a gap the last frame freezes by default, and the timeline option `enable='not(between(t,a,b))'` hides it instead. Two traps:
   - If the cam runs **past** the game's end, `overlay` keeps going with the last game frame frozen (the sync level drops to the cam). So always bound the cam with `trim=end=L` and the output with `-t L`.
   - `vstack` drops game frames until the cam starts (`before=EXT_STOP`), so **do not use `vstack` for portrait**. Use `pad` plus `overlay`.
4. **Store the cam as MP4 with real timestamps**, not as a raw elementary stream that assumes a constant frame rate. Webcams lower their frame rate in low light (Logitech RightLight), and a dropout would squash time and shift everything after it.
5. **Speed:** on NVENC the encoder is the bottleneck. NVIDIA's own table gives H.264 1080p at P5 VBR HQ as 264–317 fps per engine, and at P3 as 602–708 fps. A 30 s 1080p60 composite is therefore about 6–7 s at `p5` (what `edit.rs` uses) and about 3–4 s at `p3`. A 120 s 1440p60 composite is about 40–48 s at `p5`. "A few seconds longer" only holds for short 1080p clips with a fast preset. **Recommendation:**
   - Write the clean clip exactly as today (1–2 s).
   - Composite in the background at below-normal priority with a fast preset, and swap the result in.
   - Composite only when the cam is visible and actually has frames in the clip window. Otherwise keep `-c:v copy`.
6. **Design:** keep the clean game video, `cam.mp4` and `facecam.json` (offsets, gaps, layout) in the originals store. Produce the composite from them on save, and recomposite from them on every layout change or front trim (one generation loss, as `edit.rs` invariant 3 already requires). A hidden cam means a stream copy. Portrait is a separate export target.

---

## 1. How the repo uses ffmpeg today

### 1.1 Binary, version, provenance

| What | Where | Value |
|---|---|---|
| Version pin | `src-tauri/src/tools.rs:26` | `const VERSION: &str = "9.0.1";` |
| Download | `src-tauri/src/tools.rs:29-30` | `https://github.com/EinFabo/clippiboy/releases/download/ffmpeg-9.0.1/ffmpeg-9.0.1.zip` |
| Hashes | `src-tauri/src/tools.rs:34-43` | SHA-256 of `ffmpeg.exe` and `ffprobe.exe` "from gyan.dev's `ffmpeg-9.0.1-essentials_build`" |
| Release builder | `scripts/fetch-ffmpeg.mjs:25-32` | `VERSION = "9.0.1"`, `ffmpeg-${VERSION}-essentials_build.zip` from the GyanD/codexffmpeg GitHub mirror, pinned zip SHA-256 |
| Lookup | `src-tauri/src/muxer.rs:40-50` | the app's own copy beats `PATH` |
| Process | `src-tauri/src/muxer.rs:52-60` | `Command` with `CREATE_NO_WINDOW` (`0x0800_0000`) and **normal priority** |

Two changes in FFmpeg 9.0/9.0.1 matter here (Changelog on `release/9.0`):
- 9.0: "Remove deprecated NVENC options and support for pre-11.1 SDK versions". The repo's `-preset p5 -rc vbr/cbr -cq` is unaffected: `preset` still offers `p1..p7` and `rc` still offers `constqp`, `vbr` and `cbr` (`libavcodec/nvenc_h264.c`, release/9.0).
- 9.0.1: "avfilter/vf_scale_cuda: fix non-scaling format conversion". This is exactly the NV12→YUV420P step the CUDA path in §3.3 needs, so the pinned 9.0.1 carries the fix.

### 1.2 Every place ffmpeg is called, with the literal arguments

**a) Saving a clip from the buffer.** `muxer::build` (`muxer.rs:114-226`) writes the packets as a raw Annex-B stream (`muxer.rs:139-149`, with SPS/PPS prepended) and WAVs, then calls `muxer::encode` (`muxer.rs:285-448`). Recordings call the same function (`recorder.rs:616`). The argument order, `muxer.rs:311-394`:

```
-y -hide_banner -loglevel error -progress pipe:1 -nostats
-f h264 -r <rate>  -i <clip>_<id>.part.h264          # rate = frames*1e6/span_us or nominal fps (muxer::timing, :251-264)
[-f s16le -ar 48000 -ac 2]  -i <track>.wav  …        # one per audio input; s16le only for raw recordings
[-filter_complex "<stems::mix_filter>"]             # [1:a]volume=…[m0];…amix=inputs=N:normalize=0:dropout_transition=0[aout]  (stems.rs:342-371)
-map 0:v:0
  -map [aout] -c:a aac -b:a 192k -aac_coder fast -metadata:s:a:0 title=Mix -metadata:s:a:0 handler_name=Mix
  | -an
-c:v copy
-avoid_negative_ts make_zero
<clip>_<id>.part.mp4                                 # no +faststart on purpose (:357-360)
[-map k:a -c:a aac -b:a 192k -aac_coder fast -movflags +faststart <tracks>/<id>/<k-1>.m4a …]   # per-source stems when >1 input
```
The file is then checked with `ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1` (`muxer.rs:529-545`) and renamed into place (`muxer.rs:455-470`).

**b) Edit (mix and trim).** `edit::arguments` (`edit.rs:632-737`):
```
-y -hide_banner -loglevel error -progress pipe:1 -nostats
[-ss <video_start s>] -i <video: clip file or originals/<id>/video.mp4>   # -ss only when video_start_ms > 0 (:670-675)
[-ss <stems_start s>] -i <stem 0.m4a> …                                   # every stem gets its own -ss (:677-686)
[-filter_complex "<mix_filter>"]
-map 0:v:0 (-map [aout] -c:a aac -b:a 192k -metadata… | -an)
( <video_args> | -c:v copy )                                               # re-encode only when cutting the front (:724-728)
-avoid_negative_ts make_zero -movflags +faststart
-t <length s> <clip>.<pid>.neu.mp4
```
`video_args` (`edit.rs:768-798`), with Q = `quantiser(quality)` = clamp(round(51 − 0.44·quality), 14, 28), so the default quality 70 gives **Q = 20** (`edit.rs:757-760`, `model.rs:183-185`):

| Encoder | Literal arguments |
|---|---|
| NVENC | `-c:v h264_nvenc -preset p5 -rc vbr -pix_fmt yuv420p -cq Q -b:v 0` |
| AMF | `-c:v h264_amf -quality balanced -rc cqp -pix_fmt yuv420p -qp_i Q -qp_p Q` |
| QSV | `-c:v h264_qsv -preset medium -pix_fmt yuv420p -global_quality Q` |
| x264 | `-c:v libx264 -preset veryfast -pix_fmt yuv420p -crf Q` |

When the hardware encoder fails, the edit retries with x264 (`edit.rs:459-484`). Progress comes from parsing `out_time=` (`edit.rs:804-851`).

**c) Export to a target size.** `export::arguments` (`export.rs:116-155`), called from `commands::shrink` (`commands.rs:1151-1215`, which also falls back to x264 at `:1184-1205`):
```
-y -hide_banner -loglevel error -progress pipe:1 -nostats -i <clip>
-map 0:v:0 -map 0:a:0?  [-vf scale=-2:<h>]
-c:v h264_nvenc -preset p5 -rc cbr | h264_amf -quality balanced -rc cbr | h264_qsv -preset medium | libx264 -preset medium
-b:v <k>k -maxrate <1.1k>k -bufsize <2k>k -pix_fmt yuv420p -c:a aac -b:a 128k -movflags +faststart <out>
```

**d) Smaller calls:**
- Thumbnails: `-ss 0.5 -i <video> -frames:v 1 -vf scale=480:-1 -q:v 4` (`thumbs.rs:32-37`).
- Waveform: `showwavespic` (`preview.rs:42-58`).
- Share-link faststart remux: `-map 0 -c copy -movflags +faststart` (`links.rs:504-511`).
- Stem cuts for sending: `-ss … -i … -t … -c copy` (`share.rs:1157-1162`).

**e) Encoders:**
- Ids: `Nvenc`, `Amf`, `Qsv`, `X264` (`encode.rs:24-62`). Availability is read from the Media Foundation registry (`mft.rs`).
- **`h264_mf` is never used.** `grep h264_mf` finds nothing, although the build has `--enable-mediafoundation`.
- The encoder for edits and exports is `encode::effective(recording.encoder)` with `recording.quality` (`commands.rs:1362-1365`).
- Recording parameters that affect us: 60 fps by default (`config.rs:33`) and a 2 s keyframe interval (`config.rs:37`). The offered rates are `[30, 60, 120]` plus the monitor's refresh rate, so 144/165/240 fps are possible (`Recording.tsx:17-31`).

### 1.3 What the pinned build contains

gyan.dev lists the hardware support "in all builds": `amf, cuda, cuvid, d3d11va, d3d12va, dxva2, libvpl, nvdec, nvenc, vaapi`. **Vulkan, OpenCL and libplacebo are in the full build only** ([gyan.dev builds](https://www.gyan.dev/ffmpeg/builds/)). The configure line of a gyan essentials build carries `--enable-cuda-llvm --enable-ffnvcodec --enable-nvenc --enable-nvdec --enable-cuvid --enable-libvpl --enable-amf --enable-d3d11va --enable-dxva2 --enable-mediafoundation --enable-libx264 --enable-libzimg`, and not `--enable-vulkan`, `--enable-libplacebo` or `--enable-opencl`. That comes from a published 2024 essentials banner ([Shotstack](https://shotstack.io/learn/how-to-use-ffmpeg/)). The 9.0.2 *full* build banner adds vulkan, libplacebo and opencl ([czkawka#2110](https://github.com/qarmin/czkawka/issues/2110)). I could not read the 9.0.1 essentials banner itself, so **verify it** (§1.5).

Filter dependencies in `configure` on release/9.0, matched against that build:

| Filter | configure deps | In 9.0.1 essentials? |
|---|---|---|
| `overlay`, `alphamerge`, `geq`, `scale`, `crop`, `pad`, `hflip`, `setpts`, `trim`, `tpad`, `boxblur`, `gblur`, `split`, `format`, `vstack` | built in | yes |
| `overlay_cuda`, `pad_cuda`, `transpose_cuda` | `ffnvcodec` + any of `cuda_nvcc cuda_llvm` | yes (cuda-llvm) |
| `scale_cuda`, `hwupload_cuda` | `ffnvcodec` (+ nvcc/llvm for scale_cuda) | yes |
| `overlay_qsv`, `vpp_qsv`, `scale_qsv` | `libmfx` (provided by `--enable-libvpl`) | yes |
| `overlay_vulkan`, `scale_vulkan`, `hflip_vulkan` | `vulkan spirv_compiler` | **no** |
| `libplacebo` (offers `corner_rounding`) | libplacebo | **no** |
| `overlay_opencl` | `opencl` | **no** |
| `scale_d3d11` (8.0), `scale_d3d12` (8.1) | `d3d11va`, `d3d12va` | yes, but there is **no D3D11/D3D12 overlay** |
| `vpp_amf`, `sr_amf`, `frc_amf` | `amf` | yes, but there is **no AMF overlay** |
| `overlay_vaapi` | vaapi | Listed, but on Windows it needs Microsoft's VAOn12 runtime. Not something to rely on. |

The list of `overlay*` filters comes from `libavfilter/allfilters.c` on release/9.0: `overlay, overlay_cuda, overlay_opencl, overlay_qsv, overlay_vaapi, overlay_vulkan`.

### 1.4 Facts about the repo that shape the design

- **The watermark is burned into the game picture on the GPU, bottom-left**: 7 % of the height, 1.5 % margin (`watermark.rs:1-28`, `convert.rs:324`). A cam in the bottom-left corner covers it. A centred 9:16 crop cuts it away.
- **On AMD the encoder's lookahead runs on the shaders**, which is why low latency is on by default for AMF and QSV (`mft.rs:405-436`). For a composite encode while the game runs, do not switch on AMF `-preanalysis`.
- **Packets carry `pts_us`** (`buffer.rs:17-23`), so the cam ring can supply real timestamps for an MP4.

### 1.5 Checks for Fabi to run on the target machine

```
ffmpeg -hide_banner -buildconf
ffmpeg -hide_banner -hwaccels
ffmpeg -hide_banner -filters | findstr /i "overlay alphamerge geq scale_cuda pad_cuda hwupload qsv vulkan placebo"
ffmpeg -hide_banner -h filter=overlay_cuda
ffmpeg -hide_banner -h encoder=h264_nvenc   (and h264_amf, h264_qsv, h264_mf)
```

---

## 2. Cookbook

### 2.0 Conventions for every graph

- **Rust computes every number** (pixels, seconds) and the graph contains only literals, as with `edit::arguments` and `export::arguments`: pure functions with unit tests. Avoid `:` inside expressions. Put any expression containing commas in single quotes (filtergraph-level quoting). No shell is involved; `Command` passes each argument verbatim.
- **Input order:**
  - save path: `0` = clean game MP4 (with audio), `1` = `cam.mp4`, `2…` = mask and ring PNGs;
  - edit path: `0` = clean original, `1..n` = stems (unchanged numbering), `n+1` = cam, `n+2…` = PNGs. Putting the cam last avoids renumbering `stems::mix_filter`.
- **Even numbers:** sizes and positions must be even for yuv420. `overlay` rounds x/y down to the chroma grid anyway (`normalize_xy(...) & ~((1<<sub)-1)` in `vf_overlay.c`).
- **`eval=init`** on `overlay` because the position is constant, so the expressions are not re-evaluated every frame.
- **Output frame rate:** `overlay` emits one frame per *main* (game) frame. The main input has sync level 2 and the overlay 1 (`ff_framesync_init_dualinput`, `framesync.c`). The output therefore keeps the game's 60/120/144 fps, the 30 fps cam is simply held, and no `-fps_mode` is needed.
- **Pixel formats:** with `format=yuv420` (the default), `overlay` accepts NV12, YUV420P and YUVA420P as main input, and **only YUVA420P as the overlay input** (`main_pix_fmts_yuv420`, `overlay_pix_fmts_yuv420`). A plain yuv420p cam is converted automatically with an opaque alpha. NV12 from `-hwaccel d3d11va` can be the main input directly.
- **Cost:** only the overlapping rectangle is blended, and blending is slice-threaded with x86 SIMD row functions (`vf_overlay.c`: `imax/jmax` loop bounds, `ff_overlay_init_x86`, `AVFILTER_FLAG_SLICE_THREADS`).

### 2.a Landscape 16:9, cam in a corner

**Geometry**, with output = game size W×H (for example 1920×1080) and cam source 1280×720:

```
h  = even(round(size * H))                 size = 0.25        -> 270
w  = even(round(h * crop_w / crop_h))      16:9 cam crop       -> 480  (circle: crop square -> 270)
b  = even(border_px)                       0 without a border
m  = even(round(margin * H))               margin = 0.025      -> 26 or 28 (take 28)
fx = W - (w + 2b) - 2m                      free space         -> 1920-480-56 = 1384
fy = H - (h + 2b) - 2m                                         -> 1080-270-56 = 754
X  = m + even(round(nx * fx))              nx,ny in 0..1       -> bottom right (1,1): X=1412, Y=782
Y  = m + even(round(ny * fy))
```

**Graph (rectangle, mirrored):**
```
[1:v]setpts=PTS-STARTPTS,trim=end=30.000,hflip,scale=480:270,format=yuva420p[cam];
[0:v][cam]overlay=x=1412:y=782:eval=init:eof_action=pass[v]
```

**Full arguments for the save path**, reading the clean clip that `muxer::encode` has just written. Example: the cam started 0.412 s *before* the game (see 2.e), the clip is 30 s, NVENC.
```
-y -hide_banner -loglevel error -progress pipe:1 -nostats
-hwaccel d3d11va -i <data>/originals/<id>/video.mp4
-ss 0.412 -i <data>/originals/<id>/cam.mp4
-filter_complex "[1:v]setpts=PTS-STARTPTS,trim=end=30.000,hflip,scale=480:270,format=yuva420p[cam];[0:v][cam]overlay=x=1412:y=782:eval=init:eof_action=pass[v]"
-map [v] -map 0:a? -c:a copy
-c:v h264_nvenc -preset p5 -rc vbr -cq 20 -b:v 0 -pix_fmt yuv420p -g 120
-avoid_negative_ts make_zero
-t 30.000 <clip>_<id>.part.mp4
```
- `-hwaccel d3d11va` with no `-hwaccel_output_format` decodes on the GPU and hands frames to the CPU filters. ffmpeg's documentation says it "will usually need to copy the decoded frames from the GPU memory into the system memory" ([ffmpeg.html, -hwaccel](https://ffmpeg.org/ffmpeg.html)). Add `-hwaccel_device <dxgi index>` to stay on the recording adapter (`gpu.rs`). If hwaccel setup fails, retry without it, in the same pattern as the x264 fallback.
- `+faststart` is left out to match `muxer.rs:357-360`. The edit path keeps it.

**Cam hidden** (or no cam frame inside the clip window): **no cam input, no graph, `-c:v copy`.** The save path stays exactly what `muxer::encode` is today. In the editor see the decision table in §5.3: "hidden, no front cut" copies from the clean original with `-t`.

### 2.b Rounded rectangle and circle, and the accent border

Three ways to make the shape:

| Approach | What runs per frame | Cost (estimate) | Verdict |
|---|---|---|---|
| **1. Pre-rendered gray8 PNG plus `alphamerge`** | memcpy of the mask plane into the cam's alpha plane (130 kB at 480×270) | ~10–30 µs per cam frame | **use this** |
| 2. `geq` evaluated **once** on a one-frame `color` source, then `alphamerge` | nothing (the single mask frame repeats) | ~10–40 ms once | fine when temp files are a nuisance |
| 3. `geq` on the cam stream (per pixel, every frame) | `av_expr_eval()` for every pixel of every plane, every frame. `vf_geq.c` keeps no cache ("TODO: keep the last few frames") | ~20–45 ms per cam frame single-threaded at 480×270 (about 290 k evaluations at ~25 expression nodes each). At 30 fps that is 0.6–1.4 CPU-seconds **per clip second** | **never** |

Why the mask repeats: `alphamerge` uses dual-input framesync. The mask input has `before=EXT_NULL` and `after=EXT_INFINITY` with `repeatlast=1` by default, so a single PNG frame at pts 0 is reused for every cam frame. Inputs:
- `alphamerge` takes its main input in YUVA444P, YUVA422P, YUVA420P, GBRAP, RGBA, BGRA, ARGB or ABGR, and its alpha input in **GRAY8 only**;
- both inputs must be the **same size**, or it fails with "Input frame sizes do not match" (`vf_alphamerge.c`, `framesync.c`).

**Rendering the masks in Rust.** The `png = "0.17"` crate is already a dependency (`src-tauri/Cargo.toml:46`). Anti-aliased gray8 at exactly w×h, where 255 is opaque and centre offsets are measured from the pixel centre:
```
circle:       a = 255 * clamp(r - hypot(x+0.5-w/2, y+0.5-h/2) + 0.5, 0, 1)          r = min(w,h)/2
rounded rect: dx = max(|x+0.5-w/2| - (w/2-R), 0); dy = max(|y+0.5-h/2| - (h/2-R), 0)
              a = 255 * clamp(R - hypot(dx, dy) + 0.5, 0, 1)                        R = radius px
ring (border, RGBA, accent colour), size (w+2b)x(h+2b):
              alpha = outer_shape(R+b) - inner_shape(R)  (both AA as above), colour = accent
```

**Graph: rounded rectangle with a 4 px accent border** (mask `mask_480x270.png`, ring `ring_488x278.png`):
```
[1:v]setpts=PTS-STARTPTS,trim=end=30.000,hflip,scale=480:270,format=yuva420p[c];
[2:v]format=gray[m];
[c][m]alphamerge[cm];
[cm]pad=488:278:4:4:color=black@0[cp];
[3:v]format=yuva420p[ring];
[cp][ring]overlay=x=0:y=0:eval=init[badge];
[0:v][badge]overlay=x=1408:y=778:eval=init:eof_action=pass[v]
```
- The border sits **on the cam's timeline**: the cam is the main input of the inner `overlay`. It therefore disappears together with the cam (before its start, during hidden gaps, after its end). A ring overlaid directly onto the game would stay visible while the cam is absent.
- The inner overlay's main input (`cp`) is YUVA420P, so `main_has_alpha` is set and the badge keeps a combined alpha (`vf_overlay.c`, `config_input_main`).
- The outer position moves by −b: (1412−4, 782−4).
- Apply any `+delay` setpts **after** the badge is built, so the mask and ring frames at pts 0 stay ≤ the first cam pts.

**Graph: circle.** Crop the cam square first (centred, or at the face offset), then mask:
```
[1:v]setpts=PTS-STARTPTS,trim=end=30.000,crop=720:720:280:0,hflip,scale=270:270,format=yuva420p[c];
[2:v]format=gray[m];[c][m]alphamerge[cm];
[0:v][cm]overlay=x=1622:y=782:eval=init:eof_action=pass[v]
```

**Variant 2 without PNG files:** a one-frame source and one evaluation.
```
color=c=white:s=480x270:r=1:d=1,format=gray,geq=lum='255*clip(24-hypot(max(abs(X+0.5-W/2)-W/2+24,0),max(abs(Y+0.5-H/2)-H/2+24,0))+0.5,0,1)'[m]
```
`geq` provides `X, Y, W, H, N, SW, SH, T` and the option `lum` (alias of `lum_expr`) for GRAY8 (`vf_geq.c`).

Not available in essentials: `libplacebo`'s `corner_rounding` would do the rounding on the GPU ([cvslog](https://ffmpeg.org/pipermail/ffmpeg-cvslog/2023-May/137574.html)), but it needs the full build.

### 2.c Portrait 9:16 (1080×1920)

**Geometry.** Split s = 0.35, so the cam band is 1080×672 and the game band 1080×1248 starting at y=672.

- **Game band from a 1920×1080 game:** keep the full height and crop the band's aspect ratio. `cw = even(1080 * 1080/1248) = 934`, `cx = even((1920-934) * gx)`. With gx = 0.5 (centred) that gives 492; gx comes from the layout. Then scale to 1080×1248. That is ×1.156 up from 1080p, and ×0.867 down from 1440p, where cw = 1246.
- **Cam band from 1280×720 (cover-crop, then scale):** `ccw = even(720 * 1080/672) = 1156`, `ccx = (1280-1156)/2 = 62`. Crop first, then scale, so fewer pixels go through swscale.
- **Do not use `vstack`.** Its inputs get `before = EXT_STOP` (`vf_stack.c`), so game frames before the cam's first frame are lost, and it needs identical widths and formats. Build a canvas with `pad` and overlay the cam instead. The cam band then follows the same `eof_action` and `enable` rules as in 2.a.
- Add `setsar=1` after each non-proportional scale. `scale` compensates the aspect ratio through the SAR, so 934→1080 by 1080→1248 would leave a SAR of about 1.0007 that some players act on.

**Graph: cam on top, game below, band filled with colour when the cam is absent:**
```
[0:v]crop=934:1080:492:0,scale=1080:1248,setsar=1,pad=1080:1920:0:672:color=black[base];
[1:v]setpts=PTS-STARTPTS,trim=end=8.000,crop=1156:720:62:0,hflip,scale=1080:672,setsar=1[band];
[base][band]overlay=x=0:y=0:eval=init:eof_action=pass[v]
```

**Graph: blurred game behind the cam band** (for the time the cam is absent). Only the band is blurred, so the cost stays small:
```
[0:v]split=2[g1][g2];
[g1]crop=1736:1080:92:0,scale=270:168,boxblur=lr=10:lp=2,scale=1080:672,setsar=1[blur];
[g2]crop=934:1080:492:0,scale=1080:1248,setsar=1,pad=1080:1920:0:672[gb];
[gb][blur]overlay=x=0:y=0:eval=init[base];
[1:v]setpts=PTS-STARTPTS,trim=end=8.000,crop=1156:720:62:0,hflip,scale=1080:672,setsar=1[band];
[base][band]overlay=x=0:y=0:eval=init:eof_action=pass[v]
```
`1736 = even(1080 * 1080/672)` is the game crop with the band's aspect ratio.

**Fallback: game only with a blurred background fill** (no cam, or the cam is off):
```
[0:v]split=2[a][b];
[a]scale=270:480:force_original_aspect_ratio=increase,crop=270:480,boxblur=lr=10:lp=2,scale=1080:1920,setsar=1[bg];
[b]scale=1080:-2,setsar=1[fg];
[bg][fg]overlay=x=0:y=656:eval=init[v]
```
- For 1920×1080, `fg` is 1080×608 and `y = (1920-608)/2 = 656`.
- The blur runs at 270×480 (cheap). The final upscale to 1080×1920 is the main cost, a few ms per frame (*estimate*).
- A sharper alternative without bars is a 9:16 centre crop: `crop=608:1080:656:0,scale=1080:1920,setsar=1`.

**Full arguments for the portrait export.** This is a separate file. It uses the existing size-target recipe and bitrate mode from `export.rs`. The audio comes from the current clip file, which already holds the trimmed mix.
```
-y -hide_banner -loglevel error -progress pipe:1 -nostats
-hwaccel d3d11va -ss 4.000 -i <data>/originals/<id>/video.mp4
-ss 4.412 -i <data>/originals/<id>/cam.mp4
-i <clips>/<clip>.mp4
-filter_complex "<one of the portrait graphs above>"
-map [v] -map 2:a:0?
-c:v h264_nvenc -preset p5 -rc cbr -b:v 9000k -maxrate 9900k -bufsize 18000k -pix_fmt yuv420p
-c:a aac -b:a 128k -movflags +faststart -t 8.000 <out>_9x16.mp4
```
If the recipe starves 1080×1920, step down to 720×1280 by scaling every number with the same arithmetic.

Note: the burned-in watermark sits bottom-left in the 16:9 picture, so a centred 9:16 crop loses it. If the mark is wanted in portrait, overlay `assets/watermark.png` again in the export (open question).

### 2.d Mirroring

`hflip` goes on the **cam chain only**. It copies a small frame and costs nothing measurable. Two points:
- **Do the crop in source coordinates before `hflip`.** If the UI shows the mirrored cam and the user moves the crop window there, convert with `x_src = iw - x_ui - crop_w`.
- **The virtual camera stays unmirrored** (spec, "passes the real camera through unchanged"), so mirroring exists only in the composite, which is consistent.

### 2.e Time alignment, dropouts, and why the cam should be an MP4

**Coordinates.** Everything is in *original* coordinates, with the game's first frame of the clean original = 0. Its QPC is `snapshot.start_100ns`, which the audio is already cut to (`pipeline.rs:574-576`).
- `d = (cam_first_frame_qpc − game_first_frame_qpc) + user_delay`. The cam ring is cut at the cam keyframe at or before the game's start, so **d is normally negative**, by up to one cam GOP. Use a 1 s cam GOP.
- Trim excerpt = `[a, a+L)` in original coordinates (`ClipOriginal.start_ms` and the length).

**The arithmetic, in Rust:**
```
e = a - d                      # where the excerpt starts inside cam.mp4
if e > 0:  cam input gets "-ss e"   and no delay
else:      no -ss on the cam        and "setpts=PTS-STARTPTS+(-e)/TB"   (equivalent: -itsoffset (-e))
game input gets "-ss a" when a > 0; the output gets "-t L"
cam chain always gets "trim=end=L" (after the setpts)
```
Never put `-ss` and `-itsoffset` on the same input. One or the other keeps the behaviour obvious and testable.

**What `overlay` does at the edges.** These are framesync semantics from `libavfilter/framesync.{c,h}` on release/9.0.

| Situation | Behaviour | Why |
|---|---|---|
| Game frames before the cam's first frame | game passes through, no cam | secondary `before = EXT_NULL` ("Ignore this stream and continue processing the other ones"); `do_blend` returns the main frame when `!second` |
| Cam ends early (`eof_action=pass` or `repeatlast=0`) | cam disappears after its last frame | `pass` sets the secondary `after = EXT_NULL` (`ff_framesync_configure`) |
| Cam ends early, default (`eof_action=repeat`, `repeatlast=1`) | last cam frame frozen to the end | `after = EXT_INFINITY` |
| **Cam runs past the game's end** | **output continues with the last game frame frozen** until the cam ends | When main hits EOF its sync drops to 0, the sync level falls to the cam (1) and cam frames drive further events, while main is `EXT_INFINITY` (`framesync_inject_status`, `framesync_sync_level_update`). **Guard with `trim=end=L` on the cam and `-t L` on the output.** |
| `shortest=1` | ends when *either* input ends | Sets `after = EXT_STOP` for all inputs. It would cut the clip when the cam drops out at the end, so **do not use it.** |
| Gap mid-stream (no cam frames for a while) | last frame before the gap frozen | `ts_sync_mode=default` takes the "Frame from secondary input with the nearest lower or equal timestamp" |
| Gap that should be hidden | `enable='not(between(t,g0,g1)+between(t,g2,g3))'` on the overlay | `overlay` has `AVFILTER_FLAG_SUPPORT_TIMELINE_INTERNAL`; `ff_framesync_dualinput_get` sets the secondary frame to NULL when `ctx->is_disabled` |

**Gap policy (suggestion).** At save time Rust finds gaps from the cam's `pts_us` (Δ larger than 250 ms). Gaps shorter than 0.5 s freeze (the default, and calmer than flicker). Gaps of 0.5 s or more are stored in `facecam.json` in original coordinates and hidden with `enable`, using `t = g − a` clipped to `[0, L)`. In portrait, a hidden band shows the colour or blur fill from 2.c.

**Options compared:**
- `-itsoffset off` (input option): "Specifying a positive offset means that the corresponding streams are delayed by the time duration specified in offset" ([ffmpeg.html](https://ffmpeg.org/ffmpeg.html)). Equivalent to the setpts delay. Fine, but not together with `-ss` on the same input.
- `setpts=PTS-STARTPTS+X/TB`: explicit and visible in the graph, so unit tests can check it. **Recommended.** Put it *first* in the cam chain, so masks with pts 0 line up and the queued frames ahead of `overlay` are the small, scaled ones.
- `tpad=start_duration=X:start_mode=clone`: only for "show the first cam frame while the cam has not started yet". It times the padded frames from the link's frame rate (`vf_tpad.c`), which is a poor fit for a variable-rate cam. Prefer the colour or blur fill.
- `eof_action=pass`: yes. `repeatlast`: leave at 1, because `pass` overrides it for the end. `shortest`: no (see above).

**Store the cam as MP4 with real timestamps, not as a raw elementary stream.**
- A raw `.h264` file carries no timestamps. ffmpeg spreads the frames at `-r`, which is what `muxer::timing` does for the game (`muxer.rs:228-264`). For the game that is safe, because the capture clock paces the frames.
- Webcams are not paced. Logitech's RightLight "adjusts the frame rate, increasing the exposure time" in low light ([Logitech RightLight 2 brief](https://download01.logitech.com/web/ftp/pub/pdf/cameras/rightLight2_innovation_brief_enu.pdf)), and C920 users measure 13–15 fps when 30 is asked for ([OpenCV forum](https://forum.opencv.org/t/opencv-camera-low-fps/567)).
- Under a constant-rate assumption, 10 s at 24 fps read as 30 fps comes out as 8 s, so the cam ends up 2 s off. A dropout of g seconds moves everything after it g seconds early. `frames/span` (the `muxer::timing` trick) only fixes the average, not local drift or gaps.
- An MP4 with per-sample times keeps each frame where it was captured. `overlay` then needs nothing beyond the offset d.

**How to write `cam.mp4` from Rust** (on save, milliseconds of work, no ffmpeg run):
- **Option 1, portable (later Linux): the `mp4` crate** (0.14.0). `Mp4Writer::write_start`, `add_track(&TrackConfig)`, `write_sample(track_id, &Mp4Sample{start_time, duration, rendering_offset, is_sync, bytes})`, `write_end` ([docs.rs](https://docs.rs/mp4/latest/mp4/struct.Mp4Writer.html)). Samples must be **AVCC** (4-byte big-endian NAL lengths) instead of Annex-B start codes, with SPS/PPS in the `avcC` box (`AvcConfig`). Use a 90 kHz timescale, `start_time = pts_us − first_pts_us`, and `duration` = the difference to the next frame.
- **Option 2, Windows only: Media Foundation Sink Writer in pass-through.** "The input format does not have to match the target format … If the formats do not match, the method attempts to load an encoder" ([IMFSinkWriter::SetInputMediaType](https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/nf-mfreadwrite-imfsinkwriter-setinputmediatype); the sink writer hosts "optionally one or more encoders", per [Sink Writer](https://learn.microsoft.com/en-us/windows/win32/medfound/sink-writer)). With matching H.264 types it writes the samples as they are. Untested here.
- **Either way:**
  - start at a keyframe;
  - no B-frames on the cam encoder, so pts = dts;
  - tag the colour (VUI). If the cam is BT.601 and the game BT.709, add `scale=…:in_color_matrix=bt601:out_color_matrix=bt709` to the cam chain. The `scale` option names were checked in `vf_scale.c`;
  - if the cam encoder is **re-created** mid-buffer (camera re-plugged, other resolution), start a new segment `cam.1.mp4` with its own offset, and chain one `overlay` per segment, each with `eof_action=pass`.

### 2.f Frame-accurate trim of the composite

- **Use input `-ss` on every input** (`-ss a` on the clean original, `-ss e` on the cam, `-ss a` on the stems), **not the `trim` filter**. When transcoding, "seeking is accurate" by default (`-accurate_seek`): ffmpeg seeks to the keyframe before the position, and "this extra segment between the seek point and position will be decoded and discarded" ([ffmpeg.html](https://ffmpeg.org/ffmpeg.html)). `trim=start=a,setpts=PTS-STARTPTS` gives the same frames but decodes from the start of the file, so a trim near the end of a two-minute original decodes two minutes for nothing.
- **Always composite from the original**, with absolute offsets from `plan()` (`video_start_ms = absolute_start` when `VideoSource::Original`). That matches invariant 3 in `edit.rs:25-27`: one generation, however often the layout or trim changes.
- **The only `trim` filter in the graph is `trim=end=L` on the cam**, the safety bound from 2.e. `-t L` stays an output option after all inputs (`edit.rs:732-735`).
- **Edit arguments example:** trim 4 s → 12 s, two stems, cam visible with a rounded mask, gap [10.2, 13.7) in original coordinates, d = −0.412.
```
-y -hide_banner -loglevel error -progress pipe:1 -nostats
-hwaccel d3d11va -ss 4.000 -i <data>/originals/<id>/video.mp4
-ss 4.000 -i <data>/tracks/<id>/0.m4a
-ss 4.000 -i <data>/tracks/<id>/1.m4a
-ss 4.412 -i <data>/originals/<id>/cam.mp4
-i <data>/originals/<id>/mask_480x270.png
-filter_complex "[1:a]volume=1[m0];[2:a]volume=0.501[m1];[m0][m1]amix=inputs=2:normalize=0:dropout_transition=0[aout];[3:v]setpts=PTS-STARTPTS,trim=end=8.000,hflip,scale=480:270,format=yuva420p[c];[4:v]format=gray[k];[c][k]alphamerge[cm];[0:v][cm]overlay=x=1412:y=782:eval=init:eof_action=pass:enable='not(between(t,6.200,8.000))'[v]"
-map [v] -map [aout] -c:a aac -b:a 192k -metadata:s:a:0 title=Mix -metadata:s:a:0 handler_name=Mix
-c:v h264_nvenc -preset p5 -rc vbr -cq 20 -b:v 0 -pix_fmt yuv420p -g 120
-avoid_negative_ts make_zero -movflags +faststart
-t 8.000 <clip>.<pid>.neu.mp4
```
  The audio and video chains go into **one** `-filter_complex` string joined with `;`.
- **Do not `setpts` the game.** The clean MP4 and its audio both start at 0 (`-avoid_negative_ts make_zero` in `muxer.rs:356`). Resetting only the video PTS would shift picture against sound.

### 2.g Audio

| Path | Audio |
|---|---|
| Save-time composite (from the clean clip) | `-map 0:a? -c:a copy`: the mix AAC that `muxer::encode` just wrote, bit for bit. Stems stay where they are. |
| Edit (mix and/or trim and/or layout) | unchanged: `stems::mix_filter` → `aac 192k`, stems with their own `-ss` (`edit.rs:677-722`) |
| Export, landscape or portrait | as `export.rs`: take the audio from the **current clip file** (`-map N:a:0?`), re-encoded at 128k |
| Hidden cam, `-c:v copy` paths | unchanged |

---

## 3. Performance

### 3.1 Model

The ffmpeg 7+ command-line tool runs demuxing, decoding, filtering, encoding and muxing in separate threads. Throughput is therefore roughly the **slowest stage**, plus about 0.3–0.6 s of startup and finishing (process start, encoder setup, MP4 index). A clip has `frames = seconds × game fps`: 1800 for 30 s at 60 fps, 7200 for 120 s, and 17 280 for 120 s at 144 fps. The 30 fps cam is held, so its own decode is negligible.

| Stage | 1080p | 1440p (×1.78 px) | Source |
|---|---|---|---|
| NVDEC H.264 decode (per engine) | Ampere 748 / Ada 903 fps | ~420–510 *(scaled by pixels)* | [NVDEC app note 13.0](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvdec-application-note/index.html) |
| D3D11VA decode plus readback to the CPU | *estimate* 400–800 fps | *estimate* 250–450 | — |
| CPU H.264 decode, 6–8 cores, ~19/33 Mbit/s | *estimate* 400–700 fps | *estimate* 230–400 | — |
| CPU `overlay` of a 480×270 cam plus making the main frame writable (one full-frame copy, because decoded frames stay referenced by the decoder) | *estimate* >1500 fps | >900 | `vf_overlay.c` (blends only the overlap) |
| `alphamerge` (mask) | negligible | negligible | `vf_alphamerge.c` |
| **NVENC H.264 P5 VBR HQ (per engine)** | **264–317 fps** (Turing…Blackwell) | ~150–180 *(scaled)* | [NVENC app note 13.0](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-application-note/index.html) |
| NVENC P3 VBR HQ | 602–708 fps | ~340–400 | same |
| NVENC P1 VBR HQ | 833–948 fps | ~470–530 | same |
| AMF / QSV H.264 | no vendor table found. *Estimate* 300–600 fps at speed/balanced | ~170–340 | — |
| x264 `veryfast`, 8 cores | *estimate* 120–250 fps | 70–140 | — |

NVIDIA measured at maximum video clocks on an otherwise idle engine. In ClippiBoy the replay buffer's own encode session keeps running next to the composite, and so may the cam encoder. Expect 10–25 % less. GeForce allows 8 concurrent NVENC sessions per system from driver 551.76 on, as quoted from the SDK 12.2 note on the [NVIDIA forum](https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367). Buffer, cam and composite make three sessions.

### 3.2 What that means for the wait (CPU overlay, *estimates*)

| Clip | NVENC `p5` (today's `edit.rs`) | NVENC `p3` | NVENC `p1` | x264 `veryfast` fallback |
|---|---|---|---|---|
| 1080p60, 30 s | ~6–7 s | ~3–4 s (decode-bound) | ~2.5–3.5 s | ~8–15 s |
| 1080p60, 120 s | ~24–28 s | ~11–15 s | ~9–13 s | ~30–60 s |
| 1440p60, 30 s | ~10–12 s | ~5–6 s | ~4–5 s | ~13–26 s |
| 1440p60, 120 s | ~40–48 s | ~19–23 s | ~15–19 s | ~50–100 s |
| 1440p144, 120 s | ~100–115 s | ~45–55 s | ~35–45 s | minutes |

All of this happens **while the game is still running.** The CPU stages compete with the game, so the x264 fallback is the painful case. 4K60 is about 4× the 1080p numbers, and CPU decoding becomes the limit there, so use GPU decode.

### 3.3 GPU paths in the pinned build

**NVIDIA, everything on the GPU** (`overlay_cuda`, untested). The game never leaves VRAM. The cam is processed on the CPU, because it is tiny and because CUDA has no `hflip` or alphamerge, and is uploaded afterwards.
```
-y -hide_banner -loglevel error -progress pipe:1 -nostats
-init_hw_device cuda=gpu:0 -filter_hw_device gpu
-hwaccel cuda -hwaccel_device gpu -hwaccel_output_format cuda -i <data>/originals/<id>/video.mp4
-ss 0.412 -i <data>/originals/<id>/cam.mp4
-i <data>/originals/<id>/mask_480x270.png
-filter_complex "[0:v]scale_cuda=format=yuv420p:interp_algo=nearest[main];[1:v]setpts=PTS-STARTPTS,trim=end=30.000,hflip,scale=480:270,format=yuva420p[c];[2:v]format=gray[m];[c][m]alphamerge,hwupload[cg];[main][cg]overlay_cuda=x=1412:y=782:eof_action=pass[v]"
-map [v] -map 0:a? -c:a copy
-c:v h264_nvenc -preset p3 -rc vbr -cq 20 -b:v 0 -g 120
-avoid_negative_ts make_zero -t 30.000 <clip>_<id>.part.mp4
```
- `overlay_cuda` accepts NV12 or YUV420P as main input and NV12, YUV420P or YUVA420P as overlay. **NV12 main only matches an NV12 overlay** (`formats_match` in `vf_overlay_cuda.c`), and NVDEC delivers NV12. Hence `scale_cuda=format=yuv420p`. The fix for that non-scaling conversion is in 9.0.1.
- `overlay_cuda` takes its CUDA context **from the main input only** and does not check the overlay's. So upload with the generic `hwupload` on the *same named device* (`-filter_hw_device gpu`), not with `hwupload_cuda`, which would create its own device. `hwupload_cuda` does accept YUVA420P (`vf_hwupload_cuda.c`).
- **Leave out `-pix_fmt yuv420p`.** Frames reach NVENC as CUDA frames, and a software pixel format request would break the chain. `edit::video_args` adds `-pix_fmt` unconditionally (`edit.rs:771`), so this path needs a variant without it.
- `overlay_cuda` has `x`, `y`, `eof_action`, `eval`, `shortest` and `repeatlast`, and timeline support, so `enable=` works.
- Portrait on CUDA would need a crop. There is no `crop_cuda`. `pad_cuda` (8.0) and `scale_cuda` exist, but crop and zoom would have to go through `h264_cuvid -crop/-resize`. The portrait export is not time-critical, so **do portrait on the CPU path.**

**Intel QSV** (`overlay_qsv`, untested, more fragile):
```
-init_hw_device qsv=qs:hw_any,child_device_type=d3d11va -filter_hw_device qs
-hwaccel qsv -hwaccel_device qs -hwaccel_output_format qsv -c:v h264_qsv -i video.mp4
-ss 0.412 -i cam.mp4 -i mask_480x270.png
-filter_complex "[1:v]setpts=PTS-STARTPTS,trim=end=30.000,hflip,scale=480:270,format=yuva420p[c];[2:v]format=gray[m];[c][m]alphamerge,format=bgra,hwupload=extra_hw_frames=16[cq];[0:v][cq]overlay_qsv=x=1412:y=782:eof_action=pass[v]"
-map [v] -map 0:a? -c:a copy -c:v h264_qsv -preset veryfast -global_quality 20 -g 120 -t 30.000 out.mp4
```
- `overlay_qsv` accepts YUV420P, NV12, YUYV422, RGB32 and QSV frames. Per-pixel alpha is used when the overlay format has an alpha channel, hence `bgra`.
- QSV and software inputs **cannot be mixed**, and two QSV inputs must share one device ("Inputs with different underlying QSV devices are forbidden").
- It has a global `alpha` option (`vf_overlay_qsv.c`).
- QSV needs a fixed frame pool size, hence `hwupload=extra_hw_frames=N` (ffmpeg trac [#7006](https://trac.ffmpeg.org/ticket/7006) and [#9184](https://ffmpeg.org/pipermail/ffmpeg-trac/2021-April/058097.html)).
- Recommendation: start Intel on the CPU-overlay path (d3d11va decode, `h264_qsv` encode) and move only if measurements say so.

**AMD:** there is no GPU overlay in this build. Use d3d11va decode, CPU `overlay` and `h264_amf`, which takes system-memory frames as `edit.rs` already does. `vpp_amf` (scale) and `frc_amf` do not composite.

**Vulkan** (`overlay_vulkan`, `hflip_vulkan`, `libplacebo` with rounded corners on any vendor): **not in essentials.** Switching to the full build costs download size and needs Vulkan↔encoder interop. That is a later option, not a v1 one.

### 3.4 Decoder threads, filter threads, priority

- `-threads N` placed *before* an `-i` sets that input's decoder threads (frame threading). Limit it to 4 for the clean game while the game is running. The cam needs 1–2.
- `-filter_complex_threads N` defaults to the number of CPUs ([ffmpeg.html](https://ffmpeg.org/ffmpeg.html)). The filters work on small or few pixels, so 2 is enough.
- x264 fallback: put `-threads 4` *after* the inputs for the encoder.
- **Start the composite at below-normal priority.** In `muxer::command`, combine `CREATE_NO_WINDOW` with `BELOW_NORMAL_PRIORITY_CLASS` (`0x0000_4000`) for compositing runs. The game keeps its frame budget, and ffmpeg takes what is left.

### 3.5 Recommendation for "a few seconds longer on save"

1. **Composite only when it changes something:** the cam is visible and has at least one frame inside the clip window. Otherwise the path stays today's `-c:v copy`, which takes 1–2 s.
2. **Write the clean clip first, then composite in the background, then swap in.**
   - `muxer::encode` runs unchanged and writes the clean picture.
   - The clip row exists at once. The banner says "Clip saved, adding cam…".
   - The composite runs at below-normal priority and is moved over the clip file with `replace_file` (`muxer.rs:455-470`), exactly as `edit::swap_in` replaces files today.
   - The save is never slower than today, and a 2-minute 1440p clip does not block anything.
   - Risk: whoever drags the file out in the first seconds gets it without the cam. Mark the row as "adding cam" in the UI.
3. **Use a faster preset for save-time composites** at the same CQ (`p3` on NVENC, `-quality speed` on AMF, `-preset veryfast` on QSV, `superfast` on x264). That buys about 2× on NVENC for a few percent more bytes. Editor runs and exports keep `p5`/`balanced`/`medium`.
4. **Decode on the GPU** (`-hwaccel d3d11va` for everyone, CUDA on NVIDIA when §3.3 is adopted).
5. **Never use per-frame `geq`.** Use PNG masks.
6. **Recordings (stage 4 of the spec) can run for 30 minutes or more.** Compositing on save there means minutes, so background compositing is mandatory for them.
7. Alternative (B), lazy compositing: the in-app player shows the cam as a second `<video>` positioned with CSS, and the file is baked only on export or share. That is cheaper, but the file in the clip folder would have no cam, which breaks the spec's success criterion ("save a clip, the cam sits where it should"). Keep it as the editor *preview* technique, not as the storage model.

---

## 4. Encoder parameter mapping

The quality knob stays `recording.quality` (1–100, default 70). Q = `quantiser(quality)` = clamp(round(51 − 0.44·q), 14, 28), so 70 gives 20 (`edit.rs:757-760`). Add an explicit GOP to every composite: `-g = fps × keyframe_seconds`, which is 120 at 60 fps and 2 s (`config.rs:37`). Today `edit.rs` sets no `-g`, so the encoder defaults apply and differ (x264's default keyint is 250). Option names were checked against release/9.0 sources and docs.

| Encoder | Composite in the editor (keep consistent with `edit.rs`) | Composite on save (speed) | Notes |
|---|---|---|---|
| `h264_nvenc` | `-preset p5 -rc vbr -cq Q -b:v 0 -pix_fmt yuv420p -g G` | `-preset p3` (or `p1`), rest the same | 9.0 has `preset` p1–p7 (plus slow/medium/fast), `rc` constqp/vbr/cbr, `cq` as a float 0–51, `tune` hq by default. `-b:v 0` is required (`edit.rs:775-776`). CUDA path: **no** `-pix_fmt`. |
| `h264_amf` | `-quality balanced -rc cqp -qp_i Q -qp_p Q -pix_fmt yuv420p -g G` | `-quality speed`, rest the same | 9.0 options: `rc` cqp/cbr/vbr_peak/vbr_latency/qvbr/hqvbr/hqcbr, `qp_i`/`qp_p`/`qp_b` (−1…51), `preanalysis`, `vbaq` (`amfenc_h264.c`). Add `-qp_b Q` only if B-frames are switched on. **Do not set `-preanalysis 1`**: AMD runs the analysis on the shaders (`mft.rs:411-422`). |
| `h264_qsv` | `-preset medium -global_quality Q -pix_fmt yuv420p -g G` | `-preset veryfast` | `global_quality` without `look_ahead`/`-q` selects **ICQ** (1–51, lower is better). `-b:v`/`-maxrate` would switch to a bitrate mode ([ffmpeg-codecs, QSV ratecontrol](https://ffmpeg.org/ffmpeg-codecs.html)). |
| `libx264` | `-preset veryfast -crf Q -pix_fmt yuv420p -g G` | `-preset superfast`, `-threads 4` | The fallback, as today. |
| `h264_mf` *(not used today)* | — | `-hw_encoding 1 -rate_control quality -quality <q 0-100>` | Mirrors the recorder's `AVEncCommonQuality` (`mft.rs:318-337`) on whichever hardware MFT is registered. Options: `rate_control` default/cbr/pc_vbr/u_vbr/quality/ld_vbr/g_vbr/gld_vbr, `quality` 0–100, `hw_encoding` 0/1, `scenario` (ffmpeg-codecs). Untested. Worth a try only as a vendor-agnostic last hardware attempt before x264. |
| Export (size target, also portrait) | unchanged `export::video_args`: `-rc cbr`/bitrate with `-b:v -maxrate -bufsize` | — | — |

---

## 5. Design recommendation

### 5.1 What is stored

Use the existing originals store (`edit.rs:8-11`). Its meaning stays "the untouched recording".
```
<data>/originals/<clip-id>/video.mp4      clean game picture + recorded audio, untrimmed (what muxer::encode writes today)
<data>/originals/<clip-id>/cam.mp4        cam: H.264 in MP4, real per-frame timestamps, starts on a keyframe <= game start
<data>/originals/<clip-id>/mask_*.png     optional, regenerable from layout (gray8); ring_*.png for the border
<data>/originals/<clip-id>/facecam.json   offsets, gaps, layout
<data>/originals/<clip-id>/trim.json      as today
```
```json
{
  "version": 1,
  "cam": { "file": "cam.mp4", "width": 1280, "height": 720,
           "offset_us": -412000, "delay_us": 0, "end_us": 29950000,
           "hidden_gaps_us": [[10200000, 13700000]], "segments": ["cam.mp4"] },
  "layout": { "visible": true, "x": 1.0, "y": 1.0, "size": 0.25, "margin": 0.025,
              "shape": "rect", "radius": 0.12, "border_px": 0, "border": "#7c5cff",
              "mirror": true, "crop": { "cx": 0.5, "cy": 0.5, "zoom": 1.0 } },
  "portrait": { "split": 0.35, "game_x": 0.5, "fill": "blur" },
  "applied": { "layout_hash": "…", "start_ms": 0, "end_ms": 30000 }
}
```
Times are in original coordinates. `applied` records what the clip file currently shows, so a no-op save does not re-encode.

### 5.2 Save

1. Cut the cam ring to `[keyframe ≤ game start, game end]`, write `cam.mp4` from Rust (§2.e), and record `offset_us` and the gaps.
2. Run `muxer::encode` unchanged → the clean MP4 (1–2 s).
3. If the cam is visible and has frames in the window:
   - move the clean MP4 into the store as `video.mp4`;
   - composite into the clip path with §2.a/§2.b (fast preset, below-normal priority, background), audio `-c:a copy`;
   - swap it in, then refresh the thumbnail (`thumbs::make`).
   
   Otherwise the clean MP4 *is* the clip file, and the store holds only `cam.mp4` and `facecam.json`. `video.mp4` appears on the first composite: "the first composite files the clean picture away", the same pattern as `archives_the_clip`/`swap_in` for the first cut (`edit.rs:571-629`).
4. If the composite fails with the hardware encoder and also with x264, **keep the clean clip**, write `applied.visible = false`, and tell the user. The clip is never lost.

### 5.3 Editor rewrites: which source, copy or re-encode

| Cam visible | Front cut (a > 250 ms) | Layout changed | Video source | Video |
|---|---|---|---|---|
| no | no | — | original (clean) | **`-c:v copy`**, `-t L` |
| no | yes | — | original | re-encode, no overlay |
| yes | no | no (tail trim / mix only) | clip file (cam already baked in) | **`-c:v copy`**, `-t L` (today's fast path) |
| yes | yes / no | yes | original + cam | re-encode composite |
| yes | yes | no | original + cam | re-encode composite |

A layout change re-composites **from the clean sources**, always one generation from the recording, so it can be moved ten times without getting worse. The portrait export is a **separate export target** (§2.c). It never rewrites the clip.

### 5.4 Changes the existing `edit.rs` logic needs (it would silently go wrong otherwise)

- **`plan`/`swap_in`:** today the store is removed when nothing is trimmed (`!keep_original → remove`, `edit.rs:625-627`). With a cam the store must survive an untrimmed save: make it `keep = trimmed || has_cam`.
- **`restore_plan` (undo trim) copies the original losslessly** (`edit.rs:340-349`). With a visible cam that would **drop the cam**. It must re-composite whenever `layout.visible`.
- **`discard_original` / "Clear all trims"** (`edit.rs:159-230`) would delete the clean picture. With a cam this must mean **"bake the cam"**: delete `video.mp4`, `cam.mp4` and `facecam.json` together, and say in the UI that the cam can no longer be moved. Count `cam.mp4` in `trim_originals`.
- **`repair`** treats the note as the "alive" signal (`edit.rs:13-15`). `facecam.json` must count too, or the startup repair will clear a store it does not understand.
- **Disk:** the clean copy plus the composite is about 2× per clip, and the store lives on C: while clips may be elsewhere (`move_across`, `edit.rs:537-555`). Check free space for both (`disk.rs`).
- **`edit::video_args` needs two knobs:** a speed (save vs. edit) and "frames already on the GPU" (no `-pix_fmt`). Also add `-g`.

### 5.5 Tests to add (pure argument builders, as `edit.rs` already does)

- The cam gets `-ss e` exactly when `e > 0`, and otherwise `setpts=…+delay/TB`. Never both, and never `-itsoffset` together with `-ss`.
- `trim=end=L` is on the cam chain and `-t L` follows the last `-i`.
- A hidden cam means no cam input, no `-filter_complex` video chain, and `-c:v copy` when there is no front cut.
- All sizes and positions are even. A layout at nx = ny = 1 stays inside the frame (margin respected).
- The `enable` expression contains only gaps that intersect `[a, a+L)`, shifted by −a.
- The CUDA variant has no `-pix_fmt`, and every input index matches its `-map` and graph labels after the stems.
- `vstack` never appears.

### 5.6 To measure before committing (Fabi, on the Windows box)

1. `-filters` / `-buildconf` as in §1.5 (essentials really has cuda-llvm and libvpl).
2. A speed matrix for one 30 s and one 120 s clip at 1080p60 and 1440p60, comparing NVENC p5/p3/p1, AMF balanced/speed, QSV medium/veryfast and x264. Test with the game running, and once with `-f null -` to isolate decode plus filters. `speed=` in `-progress` gives the factor directly.
3. Sync: clap in front of the cam with the mic recording, then compare the frame of the visible clap with the audio peak. That calibrates `delay_us` per camera (the C920 and the eMeet will differ).
4. Low light on the C920 (variable frame rate) and unplugging the cam mid-buffer (gap and segment handling).

---

## Sources

- gyan.dev builds, which libraries are in essentials vs full: https://www.gyan.dev/ffmpeg/builds/
- gyan essentials configure banner (2024 build): https://shotstack.io/learn/how-to-use-ffmpeg/ ; 9.0.2 full build banner: https://github.com/qarmin/czkawka/issues/2110
- FFmpeg Changelog, release/9.0 (9.0 / 9.0.1 / 8.1 entries): https://github.com/FFmpeg/FFmpeg/blob/release/9.0/Changelog ; ffmpeg.org news (8.0, 8.1): https://ffmpeg.org/index.html#news
- FFmpeg release/9.0 sources: `configure`, `libavfilter/allfilters.c`, `framesync.c`, `framesync.h`, `vf_overlay.c`, `vf_overlay_cuda.c`, `vf_overlay_qsv.c`, `vf_alphamerge.c`, `vf_geq.c`, `vf_tpad.c`, `vf_stack.c`, `vf_scale.c`, `vf_scale_cuda.c`, `vf_hwupload_cuda.c`, `libavcodec/nvenc_h264.c`, `libavcodec/amfenc_h264.c`, all under https://github.com/FFmpeg/FFmpeg/tree/release/9.0
- ffmpeg CLI documentation (`-ss`, `-accurate_seek`, `-itsoffset`, `-t`, `-shortest`, `-hwaccel*`, `-init_hw_device`, `-filter_hw_device`, `-filter_complex_threads`): https://ffmpeg.org/ffmpeg.html
- ffmpeg codecs documentation (QSV ratecontrol selection, h264_mf options, libx264 crf): https://ffmpeg.org/ffmpeg-codecs.html
- ffmpeg filters documentation (overlay, framesync options, timeline editing, alphamerge, geq, scale, crop, pad, hflip, setpts, tpad, trim, boxblur, overlay_cuda, scale_cuda, overlay_qsv, hwupload): https://ffmpeg.org/ffmpeg-filters.html
- NVIDIA NVENC application note, SDK 13.0 (H.264 1080p fps per preset): https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-application-note/index.html
- NVIDIA NVDEC application note, SDK 13.0: https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvdec-application-note/index.html
- NVENC GeForce session limit (8 from driver 551.76, quoted from the SDK 12.2 note): https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367
- overlay_qsv usage and pitfalls: https://trac.ffmpeg.org/ticket/7006 , https://ffmpeg.org/pipermail/ffmpeg-trac/2021-April/058097.html , https://trac.ffmpeg.org/ticket/8995
- libplacebo `corner_rounding`: https://ffmpeg.org/pipermail/ffmpeg-cvslog/2023-May/137574.html
- Logitech RightLight 2 (frame rate traded for exposure): https://download01.logitech.com/web/ftp/pub/pdf/cameras/rightLight2_innovation_brief_enu.pdf ; C920 low fps report: https://forum.opencv.org/t/opencv-camera-low-fps/567
- Media Foundation Sink Writer: https://learn.microsoft.com/en-us/windows/win32/medfound/sink-writer , https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/nf-mfreadwrite-imfsinkwriter-setinputmediatype
- `mp4` crate 0.14.0: https://docs.rs/mp4/latest/mp4/struct.Mp4Writer.html , https://docs.rs/mp4/latest/mp4/struct.Mp4Sample.html
