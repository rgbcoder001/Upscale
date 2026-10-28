# upscale.vpy — anime upscale to 4K with vs-mlrt
# Called by upscale.ps1 via: vspipe -c y4m -a key=value ... upscale.vpy -
#
# Args (all passed with -a; the batch script fills these in automatically):
#   src                path to source video (required)
#   mode               upscale | passthrough | prep | selftest   (default upscale)
#                      prep = decode/IVTC/crop/trim only, 10-bit BT.709, for Topaz to upscale
#   mlrt               folder containing vsmlrt.py
#   mlrt_root          folder to search for the vs-mlrt plugin DLLs (default: mlrt)
#   model              cugan | onnx               (default cugan)
#   onnx               path to an .onnx upscaling model (when model=onnx, e.g. AnimeJaNai V3;
#                      any scale works: passes repeat until the frame reaches the target height)
#   backend            trt | cuda | dml | ncnn    (default trt)
#   noise              Real-CUGAN denoise: -1 (none), 0, or 3   (default -1)
#   matrix             source matrix: 709 | 170m | 470bg       (default 709)
#   sar                source sample aspect ratio, e.g. 32:27   (default 1:1)
#   ivtc               off | match | decimate     (default off)
#   field              tff | bff                  (default tff)
#   crop_l/r/t/b       black borders to remove before upscaling (even numbers)
#   native_height      descale to this height before upscaling; 0 = off
#   native_kernel      bilinear | bicubic | lanczos | spline36 (default bicubic)
#   native_b/native_c  bicubic b and c            (default 0 / 0.5)
#   native_taps        lanczos taps               (default 3)
#   deband             1 = light deband after upscaling        (default 0)
#   temporal           1 = temporal stabilisation              (default 0)
#   temporal_strength  0 = default for the method in use
#   height             output height              (default 2160)
#   start / frames     test-clip range (frames 0 = whole episode)

import os
import sys

import vapoursynth as vs
from vapoursynth import core


def arg(name, default=None):
    v = globals().get(name, default)
    return v.decode() if isinstance(v, bytes) else v


def log(msg):
    core.log_message(vs.MESSAGE_TYPE_INFORMATION, f"upscale.vpy: {msg}")


mode = arg("mode", "upscale").lower()
backend_name = arg("backend", "trt").lower()

# ── vs-mlrt: load its plugins from the tools folder (not VapourSynth's autoload) ──
# vsmlrt.py finds its models next to whichever of these plugins is loaded, so at least
# the one for the chosen backend must load. Every failure is reported, not swallowed.

MLRT_PLUGINS = {"vstrt.dll": "trt", "vsort.dll": "ort", "vsov.dll": "ov",
                "vsncnn.dll": "ncnn", "vstrt_rtx.dll": "trt_rtx", "vsmigx.dll": "migx"}
BACKEND_PLUGIN = {"trt": "trt", "cuda": "ort", "dml": "ort", "ncnn": "ncnn"}


def has_plugin(ns):
    try:
        getattr(core, ns)
        return True
    except AttributeError:
        return False


# prep (Topaz route) only decodes, inverse-telecines, crops and trims: no vs-mlrt needed
need_mlrt = mode != "prep"
mlrt = arg("mlrt", "")
mlrt_root = arg("mlrt_root", "") or mlrt
if mlrt and need_mlrt:
    sys.path.insert(0, mlrt)
    load_errors, found = [], []
    for root, _dirs, files in os.walk(mlrt_root):
        for name in files:
            ns = MLRT_PLUGINS.get(name.lower())
            if not ns:
                continue
            path = os.path.join(root, name)
            found.append(path)
            if has_plugin(ns):
                continue
            try:
                core.std.LoadPlugin(path)
            except vs.Error as e:
                load_errors.append(f"{path}: {e}")
    needed = BACKEND_PLUGIN.get(backend_name, "trt")
    if not has_plugin(needed):
        detail = " | ".join(load_errors) if load_errors else (
            f"found plugin files: {found}" if found else f"no vs-mlrt plugin DLLs under {mlrt_root}")
        raise vs.Error(f"upscale.vpy: vs-mlrt '{needed}' plugin (backend '{backend_name}') did not load. {detail}")
    for err in load_errors:   # other backends failing is fine, but worth a note
        core.log_message(vs.MESSAGE_TYPE_INFORMATION, f"upscale.vpy: (unused backend) {err}")

if need_mlrt:
    import vsmlrt  # noqa: E402
    from vsmlrt import Backend  # noqa: E402

# Self-test used by the batch script's tool check: plugins load, vsmlrt imports, and the
# helper filters vs-mlrt runs around its models (akarin.Expr) work.
if mode == "selftest":
    if hasattr(vsmlrt, "_expr"):
        probe = core.std.BlankClip(format=vs.RGBS, width=64, height=64, length=1)
        vsmlrt._expr(probe, "x 0.7 * 0.15 +").get_frame(0)
    log(f"vs-mlrt OK: backend {backend_name}, models in {vsmlrt.models_path}")

# ── arguments ────────────────────────────────────────────────────────────────

src = arg("src")
if not src and mode != "selftest":
    raise vs.Error("upscale.vpy: pass -a src=<path>")

model = arg("model", "cugan").lower()
onnx = arg("onnx", "")
noise = int(arg("noise", "-1"))
matrix_in = arg("matrix", "709")
target_h = int(arg("height", "2160"))
start = int(arg("start", "0"))
frames = int(arg("frames", "0"))
ivtc = arg("ivtc", "off").lower()
field = arg("field", "tff").lower()
native_h = int(arg("native_height", "0"))
native_kernel = arg("native_kernel", "bicubic").lower()
native_b = float(arg("native_b", "0"))
native_c = float(arg("native_c", "0.5"))
native_taps = int(arg("native_taps", "3"))
deband = arg("deband", "0") == "1"
temporal = arg("temporal", "0") == "1"
temporal_strength = int(arg("temporal_strength", "0"))
crop = [int(arg(f"crop_{side}", "0")) for side in "lrtb"]


def parse_sar(text):
    try:
        num, den = (float(x) for x in text.replace("/", ":").split(":"))
        return num / den if num > 0 and den > 0 else 1.0
    except ValueError:
        return 1.0


sar = parse_sar(arg("sar", "1:1"))

be = None
if need_mlrt:
    backends = {
        "trt": lambda: Backend.TRT(fp16=True, num_streams=2),  # NVIDIA, fastest
        "cuda": lambda: Backend.ORT_CUDA(fp16=True),            # NVIDIA, no engine build
        "dml": lambda: Backend.ORT_DML(fp16=True),              # AMD / Intel via DirectML
        "ncnn": lambda: Backend.NCNN_VK(fp16=True),             # any Vulkan GPU
    }
    if backend_name not in backends:
        raise vs.Error(f"upscale.vpy: unknown backend '{backend_name}'")
    be = backends[backend_name]()

# ── source ───────────────────────────────────────────────────────────────────

if mode == "selftest":
    clip = core.std.BlankClip(width=64, height=64, format=vs.YUV420P8, length=1)
else:
    clip = core.bs.VideoSource(source=src, showprogress=False)

# Inverse telecine: telecined anime (most DVDs, some older Blu-rays) is 24p film
# spread across interlaced fields. Field matching rebuilds the original frames so
# combing isn't upscaled; decimation then drops the duplicate frame in each 5.
if ivtc in ("match", "decimate"):
    if clip.format.bits_per_sample != 8:
        raise vs.Error("upscale.vpy: inverse telecine needs an 8-bit source")
    clip = core.vivtc.VFM(clip, order=0 if field == "bff" else 1, mode=1)
    if ivtc == "decimate":
        clip = core.vivtc.VDecimate(clip)
    log(f"inverse telecine: {ivtc}")

# Remove black bars (pillarbox / letterbox / overscan) so they aren't upscaled
if any(crop):
    clip = core.std.Crop(clip, *crop)
    log(f"cropped L{crop[0]} R{crop[1]} T{crop[2]} B{crop[3]} -> {clip.width}x{clip.height}")

if frames > 0:
    clip = clip[start:start + frames]

# Output size: display aspect (sample aspect ratio applied), exact target height, mod-2 width
out_w = max(2, round(clip.width * sar * target_h / clip.height / 2) * 2)


# ── stages ───────────────────────────────────────────────────────────────────

def passthrough(c):
    # No model, no colour conversion: just give the encoder 10-bit 4:2:0
    if c.format.id != vs.YUV420P10:
        c = core.resize.Point(c, format=vs.YUV420P10)
    return c


def prep(c):
    # Frames for Topaz: 10-bit 4:2:0 in BT.709 (the batch script tags them 709 for Topaz,
    # since y4m carries no colour information). Bit-depth only when already 709.
    if matrix_in != "709":
        log(f"matrix {matrix_in} -> 709 for Topaz")
        return core.resize.Spline36(c, format=vs.YUV420P10, matrix_in_s=matrix_in, matrix_s="709",
                                    dither_type="error_diffusion")
    return passthrough(c)


def descale_luma(y, w, h):
    if native_kernel == "bilinear":
        return core.descale.Debilinear(y, w, h)
    if native_kernel == "bicubic":
        return core.descale.Debicubic(y, w, h, b=native_b, c=native_c)
    if native_kernel == "lanczos":
        return core.descale.Delanczos(y, w, h, taps=native_taps)
    if native_kernel == "spline36":
        return core.descale.Despline36(y, w, h)
    raise vs.Error(f"upscale.vpy: unknown native_kernel '{native_kernel}'")


def to_model_rgb(c):
    # Spline36 for the chroma upsample into RGB: sharper than the default bicubic
    if native_h <= 0 or native_h >= c.height:
        return core.resize.Spline36(c, format=vs.RGBS, matrix_in_s=matrix_in)

    # Native-resolution descale: undo the studio's upscale to 1080p on luma, bring
    # chroma to the same size, and let the model work from the true native frames.
    nw = round(c.width * native_h / c.height / 2) * 2
    y = core.resize.Point(core.std.ShufflePlanes(c, 0, vs.GRAY), format=vs.GRAYS)
    y = descale_luma(y, nw, native_h)
    yuv = core.resize.Spline36(c, width=nw, height=native_h, format=vs.YUV444PS)
    yuv = core.std.ShufflePlanes([y, yuv], [0, 1, 2], vs.YUV)
    log(f"descaled to {nw}x{native_h} ({native_kernel})")
    return core.resize.Spline36(yuv, format=vs.RGBS, matrix_in_s=matrix_in)


def model_pass(rgb):
    if model == "cugan":
        if noise not in (-1, 0, 3):
            raise vs.Error("upscale.vpy: Real-CUGAN pro models accept noise -1, 0 or 3")
        # 3x when 2x would fall short (720p -> 2160p in one pass)
        scale = 2 if rgb.height * 2 >= target_h else 3
        return vsmlrt.CUGAN(rgb, noise=noise, scale=scale, version=2, backend=be)
    if model == "onnx":
        if not onnx:
            raise vs.Error("upscale.vpy: model=onnx needs -a onnx=<path to .onnx>")
        return vsmlrt.inference(rgb, onnx, backend=be)
    raise vs.Error(f"upscale.vpy: unknown model '{model}'")


def upscale(c):
    up = to_model_rgb(c)
    # Run the model until the frame is at least the target height (up to 3 passes),
    # so the final resize is always a downscale, never a plain upscale.
    passes = 0
    while up.height < target_h and passes < 3:
        up = model_pass(up)
        passes += 1
    log(f"{passes} model pass(es): {c.width}x{c.height} -> {up.width}x{up.height} -> {out_w}x{target_h}")

    # Output is HD, so convert to BT.709 regardless of the source matrix
    return core.resize.Spline36(
        up,
        width=out_w,
        height=target_h,
        format=vs.YUV420P10,
        matrix_s="709",
        dither_type="error_diffusion",
    )


def deband_light(c):
    # Gentle settings, no added grain (SVT-AV1 film-grain synthesis handles grain)
    for ns in ("neo_f3kdb", "f3kdb"):
        if hasattr(core, ns):
            return getattr(core, ns).Deband(c, range=24, y=32, cb=24, cr=24,
                                            grainy=0, grainc=0, output_depth=10)
    raise vs.Error("upscale.vpy: deband requested but no f3kdb plugin is installed")


def mv_stabilize(c, thsad):
    # Motion-compensated temporal smoothing with a low threshold: steadies
    # frame-to-frame shimmer from single-frame models without smearing motion.
    sup = core.mv.Super(c, pel=1, hpad=16, vpad=16)
    a = dict(blksize=32, overlap=16)
    bv1 = core.mv.Analyse(sup, isb=True, delta=1, **a)
    fv1 = core.mv.Analyse(sup, isb=False, delta=1, **a)
    bv2 = core.mv.Analyse(sup, isb=True, delta=2, **a)
    fv2 = core.mv.Analyse(sup, isb=False, delta=2, **a)
    return core.mv.Degrain2(c, sup, bv1, fv1, bv2, fv2, thsad=thsad)


def stabilize(c):
    core.max_cache_size = max(core.max_cache_size, 8000)
    try:
        from vs_temporalfix import vs_temporalfix  # pifroggi's filter, if installed
        log("temporal: vs_temporalfix")
        return vs_temporalfix(c, strength=temporal_strength or 400, tr=6)
    except (ImportError, AttributeError, vs.Error):
        log("temporal: mvtools stabiliser (vs_temporalfix not available)")
        return mv_stabilize(c, temporal_strength or 120)


# ── pipeline ─────────────────────────────────────────────────────────────────

if mode == "selftest":
    out = clip
elif mode == "passthrough":
    out = passthrough(clip)
elif mode == "prep":
    out = prep(clip)
elif mode == "upscale":
    out = upscale(clip)
    if deband:
        out = deband_light(out)
    if temporal:
        out = stabilize(out)
else:
    raise vs.Error(f"upscale.vpy: unknown mode '{mode}'")

out.set_output()
