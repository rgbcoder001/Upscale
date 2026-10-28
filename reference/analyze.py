"""analyze.py — per-file source analysis for upscale.ps1

Run with the same Python that has VapourSynth installed:
    python analyze.py --src <file> [--native] [--noise] [--ivtc off|match|decimate] [--field tff|bff]
                      [--crop left,right,top,bottom] [--frames 5]

Prints one JSON object on the last line of stdout:
    {
      "samples": [...frame numbers...],
      "noise": {"metric": 0.42, "level": -1},
      "native": {"height": 810, "width": 1440, "kernel": "bicubic", "b": 0.0, "c": 0.5, "taps": 3,
                 "score": 0.82, "prominence": 0.18, "agree": 5, "frames": 5,
                 "confident": true, "reason": "..."},
      "seconds": 31.2
    }

Native resolution: many anime are drawn below 1080p and upscaled for the Blu-ray. Descaling a
frame to its true native height with the right kernel and scaling it back reproduces the frame
almost exactly, so the reconstruction error shows a sharp dip at that height. A descale is only
recommended when the dip clearly stands out from every other height (measured against each
height's immediate neighbours, which cancels grain/compression noise) AND most frames agree.

Noise: median of the residual (frame minus its 3x3 median) measured only in flat areas, where
line art can't inflate it. Reported in 8-bit code values.
"""

import argparse
import json
import statistics
import sys
import time

import vapoursynth as vs
from vapoursynth import core

# Thresholds (logged by the batch script so they can be tuned).
# NOISE_LOW / NOISE_HIGH are mirrored in upscale.ps1 (season consensus); change both.
NOISE_LOW = 0.8          # below: no denoise
NOISE_HIGH = 1.6         # above: strong denoise
PROMINENCE_MIN = 0.06    # native dip must beat the next-best height's score by this much (true 1080p tests ~0.03)
AGREE_MIN = 0.6          # share of sampled frames whose own dip lands on the same height
EDGE_CROP = 16           # ignore borders, where descaling is least accurate

KERNELS = {
    "bilinear":         dict(kernel="bilinear"),
    "bicubic_catrom":   dict(kernel="bicubic", b=0.0, c=0.5),
    "bicubic_mitchell": dict(kernel="bicubic", b=1 / 3, c=1 / 3),
    "bicubic_sharp":    dict(kernel="bicubic", b=0.0, c=1.0),
    "lanczos3":         dict(kernel="lanczos", taps=3),
    "spline36":         dict(kernel="spline36"),
}
SCAN_FRAMES = 3          # frames used for the full height scan (agreement is checked on all samples)


def descale(y, w, h, k):
    if k["kernel"] == "bilinear":
        return core.descale.Debilinear(y, w, h)
    if k["kernel"] == "bicubic":
        return core.descale.Debicubic(y, w, h, b=k["b"], c=k["c"])
    if k["kernel"] == "lanczos":
        return core.descale.Delanczos(y, w, h, taps=k["taps"])
    return core.descale.Despline36(y, w, h)


def rescale(y, w, h, k):
    if k["kernel"] == "bilinear":
        return core.resize.Bilinear(y, w, h)
    if k["kernel"] == "bicubic":
        return core.resize.Bicubic(y, w, h, filter_param_a=k["b"], filter_param_b=k["c"])
    if k["kernel"] == "lanczos":
        return core.resize.Lanczos(y, w, h, filter_param_a=k["taps"])
    return core.resize.Spline36(y, w, h)


def native_width(src_w, src_h, h):
    # Same rounding the upscale script uses
    return round(src_w * h / src_h / 2) * 2


def errors_for(samples, heights, k, vertical_only=False):
    """Return {height: [per-frame error, ...]} for one kernel.

    vertical_only descales height alone (width untouched): scalers are separable, so the
    dip at the native height shows up the same way at roughly half the cost.
    """
    sw, sh = samples.width, samples.height
    ref = core.std.Crop(samples, EDGE_CROP, EDGE_CROP, EDGE_CROP, EDGE_CROP)
    clips = []
    for h in heights:
        w = sw if vertical_only else native_width(sw, sh, h)
        re = rescale(descale(samples, w, h, k), sw, sh, k)
        re = core.std.Crop(re, EDGE_CROP, EDGE_CROP, EDGE_CROP, EDGE_CROP)
        clips.append(core.std.PlaneStats(ref, re))
    # One long clip so frames are rendered in parallel
    big = core.std.Splice(clips)
    n = samples.num_frames
    out = {h: [] for h in heights}
    for i, f in enumerate(big.frames()):
        out[heights[i // n]].append(f.props["PlaneStatsDiff"])
    return out


def local_scores(err):
    """Error at each height relative to its immediate neighbours (3-8 rows either side).

    Absolute error trends smoothly down toward the source height and, with grain or
    compression noise, the noise floor swamps the native dip. Comparing each height with
    its close neighbours cancels both, leaving the sharp dip at the true native height.
    """
    scores = {}
    for h in err:
        left = [err[x] for x in range(h - 8, h - 2) if x in err]
        right = [err[x] for x in range(h + 3, h + 9) if x in err]
        if len(left) < 3 or len(right) < 3:
            continue
        base = min(statistics.fmean(left), statistics.fmean(right))
        scores[h] = err[h] / base if base > 0 else 1.0
    return scores


def best_dip(scores):
    """(height, score, prominence) where prominence = next-best score (2+ rows away) - best."""
    ranked = sorted(scores.items(), key=lambda kv: kv[1])
    h, score = ranked[0]
    second = next((v for x, v in ranked if abs(x - h) > 2), score)
    return h, score, second - score


def analyze_native(samples):
    sh = samples.height
    heights = list(range(int(sh * 0.5), sh - 10))
    scan = samples[:min(SCAN_FRAMES, samples.num_frames)]

    # 1. Height scan with every kernel (vertical only, few frames); keep the clearest dip
    best = None
    for name, k in KERNELS.items():
        per = errors_for(scan, heights, k, vertical_only=True)
        h, score, prom = best_dip(local_scores({x: statistics.fmean(v) for x, v in per.items()}))
        if best is None or prom > best["prominence"]:
            best = dict(height=h, scan_kernel=name, score=score, prominence=prom)

    # 2. Agreement: does each sampled frame show its own dip at the same height?
    h = best["height"]
    window = [x for x in range(h - 40, h + 41) if x in heights]
    per = errors_for(samples, window, KERNELS[best["scan_kernel"]], vertical_only=True)
    agree = 0
    for i in range(samples.num_frames):
        fs = local_scores({x: per[x][i] for x in window})
        fh = min(fs, key=fs.get) if fs else None
        if fh is not None and abs(fh - h) <= 1:
            agree += 1

    # 3. Pick kernel and exact height with a full 2D descale, as the upscale script does it
    near = [x for x in (h - 1, h, h + 1) if x in heights]
    best_k = None   # (mean error, kernel name, height)
    for name, k in KERNELS.items():
        for hh, v in errors_for(samples, near, k).items():
            m = statistics.fmean(v)
            if best_k is None or m < best_k[0]:
                best_k = (m, name, hh)
    _, kname, final_h = best_k

    k = KERNELS[kname]
    agree_ok = agree / samples.num_frames >= AGREE_MIN
    confident = best["prominence"] >= PROMINENCE_MIN and agree_ok
    if best["prominence"] < PROMINENCE_MIN:
        reason = f"no clear dip (prominence {best['prominence']:.3f} < {PROMINENCE_MIN})"
    elif not agree_ok:
        reason = f"frames disagree ({agree}/{samples.num_frames} match)"
    else:
        reason = "clear, consistent dip"

    return dict(
        height=final_h,
        width=native_width(samples.width, samples.height, final_h),
        kernel=k["kernel"], b=k.get("b", 0.0), c=k.get("c", 0.5), taps=k.get("taps", 3),
        kernel_name=kname, score=round(best["score"], 4), prominence=round(best["prominence"], 4),
        agree=agree, frames=samples.num_frames,
        confident=bool(confident), reason=reason,
    )


def analyze_noise(samples):
    med = core.std.Median(samples)
    resid = core.std.Expr([samples, med], "x y - abs")
    flat = core.std.Expr([core.std.Sobel(samples)], "x 0.02 < 1 0 ?")
    masked = core.std.Expr([resid, flat], "x y *")
    a = core.std.PlaneStats(masked)
    b = core.std.PlaneStats(flat)
    vals = []
    for fa, fb in zip(a.frames(), b.frames()):
        frac = fb.props["PlaneStatsAverage"]
        if frac > 0.05:
            vals.append(fa.props["PlaneStatsAverage"] / frac * 255)
    if not vals:
        return dict(metric=None, level=-1)
    metric = statistics.median(vals)
    level = -1 if metric < NOISE_LOW else (0 if metric < NOISE_HIGH else 3)
    return dict(metric=round(metric, 3), level=level)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--native", action="store_true")
    ap.add_argument("--noise", action="store_true")
    ap.add_argument("--ivtc", default="off")
    ap.add_argument("--field", default="tff")
    ap.add_argument("--frames", type=int, default=5)
    ap.add_argument("--crop", default="0,0,0,0", help="left,right,top,bottom black borders to remove")
    args = ap.parse_args()

    t0 = time.time()
    clip = core.bs.VideoSource(source=args.src, showprogress=False)
    if args.ivtc in ("match", "decimate") and clip.format.bits_per_sample == 8:
        clip = core.vivtc.VFM(clip, order=0 if args.field == "bff" else 1, mode=1)
        if args.ivtc == "decimate":
            clip = core.vivtc.VDecimate(clip)

    left, right, top, bottom = (int(x) for x in args.crop.split(","))
    if left or right or top or bottom:
        clip = core.std.Crop(clip, left, right, top, bottom)

    luma = core.resize.Point(core.std.ShufflePlanes(clip, 0, vs.GRAY), format=vs.GRAYS)

    # Sample the most detailed frames from the middle 70% (skips OP/ED and black frames)
    n = luma.num_frames
    candidates = sorted({int(n * p / 20) for p in range(3, 18)})
    detail = core.std.PlaneStats(core.std.Sobel(luma))
    scored = sorted(((detail.get_frame(p).props["PlaneStatsAverage"], p) for p in candidates), reverse=True)
    picks = sorted(p for _, p in scored[:max(1, args.frames)])
    samples = core.std.Splice([luma[p] for p in picks])

    result = {"samples": picks}
    if args.noise:
        result["noise"] = analyze_noise(samples)
    if args.native:
        result["native"] = analyze_native(samples)
    result["seconds"] = round(time.time() - t0, 1)
    print(json.dumps(result))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # report as JSON so the batch script can log it and carry on
        print(json.dumps({"error": f"{type(e).__name__}: {e}"}))
        sys.exit(1)
