"""Framebuffer capture: replay an input slot frame-by-frame in a headless
RustRetro instance, grabbing `app://screen` (a PNG) every landed frame, then
assemble the frames into a shareable GIF with optional caption overlays.

## The replay protocol this module encodes (verified live, port 4027)

`app://screen` returns the CURRENT framebuffer as a PNG; `McpClient` already
supports it (`read_resource("app://screen")` / `screenshot(path)`) — nothing
new needed there.

Frame-exact replay of a `shadow/inputs/<family>/<name>.slot.json` slot:

  1. `enable_writes` — required once per MCP session before any write tool.
  2. `load_state(path=..., pause_after=True)` — ATOMIC load-and-pause (the
     load and the pause happen in one lock scope on the emulation thread;
     never bracket with `resume`/`pause`, which reopens a free-running
     window the atomic form exists to close).
  3. `play_inputs(action="start", name=<bare slot name>, port="both",
     trigger="manual")` — this only ARMS the playback. Slot index 0 is
     applied to the emulated frame that lands on the FIRST `step()` issued
     after arming (`src/playback.rs`: `playback::tick` runs at the END of
     `run_frame`, so it folds into the *next* frame).
  4. Per slot frame: `step()` (synchronous — the response says whether the
     frame LANDED), then grab `app://screen`.
  5. ONE extra `step()` after the slot's last frame: this is where the
     playback's input RELEASE lands and the playback itself clears. Grabbed
     too, as a trailing bookend.

`press_buttons` never enters this picture — the slot drives every frame's
input, and it is banned repo-wide regardless (decays on GUI frames).

## GIF timing

MK2 arcade's native rate is ~54.706 Hz (~18.28 ms/frame). GIF stores
per-frame duration in CENTISECONDS, so `frame_durations_cs` dithers the
per-frame duration (accumulate-and-round against the running total) so the
GIF's TOTAL playback time matches `n_frames / fps_native` — a constant
20 ms/frame would run the clip ~9% slow.
"""

from __future__ import annotations

import argparse
import json
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Union

from PIL import Image, ImageDraw, ImageFont

from .mcpclient import McpClient

__all__ = [
    "NATIVE_FPS_MK2",
    "CaptureError",
    "Caption",
    "frame_durations_cs",
    "captions_for_frame",
    "capture_slot",
    "assemble_gif",
    "main",
]

#: MK2 arcade's native refresh rate — CLAUDE.md/the A2 brief: 54.71 Hz,
#: ~18.28 ms/frame. The one place a per-port fact like this belongs is a
#: named default a caller can override, never a silent constant baked into
#: the timing math itself (`frame_durations_cs` takes `fps` as a parameter).
NATIVE_FPS_MK2 = 54.706


class CaptureError(RuntimeError):
    """A capture-pipeline step failed in a way that voids the run — a tool
    call errored, a `step()` did not land, or an atomic load did not report
    `paused=true`."""


# ── captions ──────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Caption:
    """One caption banner, active over an inclusive frame range.

    `start_frame`/`end_frame` index into the ASSEMBLED frame list (the same
    order `capture_slot` returns / `assemble_gif` receives — bookends
    included), not into the slot's own frame numbering; a caller aligning
    captions to a beat sheet's slot-relative frames must add `capture_slot`'s
    one-frame pre-roll bookend.

    This is deliberately the only shape this module knows: the beat-sheet
    schema that will actually generate caption lists does not exist yet
    (that is A3's job), so `capture_slot`/`assemble_gif` accept plain
    `{start_frame, end_frame, text}` dicts and do not import anything
    beat-sheet-shaped.
    """

    start_frame: int
    end_frame: int
    text: str

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "Caption":
        return cls(
            start_frame=int(d["start_frame"]),
            end_frame=int(d["end_frame"]),
            text=str(d["text"]),
        )


def _coerce_captions(captions: Optional[Sequence[Union["Caption", Dict[str, Any]]]]) -> List[Caption]:
    return [c if isinstance(c, Caption) else Caption.from_dict(c) for c in (captions or [])]


def captions_for_frame(captions: Sequence[Caption], frame_index: int) -> List[str]:
    """The text of every caption active (inclusive) at `frame_index`, in the
    order given — a frame with more than one active caption stacks them."""
    return [c.text for c in captions if c.start_frame <= frame_index <= c.end_frame]


# ── GIF timing ────────────────────────────────────────────────────────────


def frame_durations_cs(n_frames: int, *, fps: float = NATIVE_FPS_MK2) -> List[int]:
    """Per-frame GIF durations in CENTISECONDS (the GIF format's own duration
    unit), one entry per frame, such that `sum(result)` is the nearest
    centisecond to `n_frames * 100 / fps` — the exact total playback time
    `n_frames` frames take at `fps`.

    Standard error-accumulation (Bresenham-style) dithering: frame `i`'s
    duration is `round((i+1) * cs_per_frame) - round(i * cs_per_frame)`, i.e.
    each frame gets whatever rounding the RUNNING total needs, rather than
    every frame independently rounding the same fractional value the same
    way. A naive constant `round(cs_per_frame)` per frame would drift the
    total by up to `n_frames` centiseconds; this drifts by at most 1.

    Every duration is clamped to a minimum of 1 centisecond — a 0 is not a
    legal GIF frame duration and several viewers substitute their own
    default (often 100 ms) for it, which would silently un-dither the whole
    sequence at playback time.
    """
    if n_frames <= 0:
        return []
    if fps <= 0:
        raise ValueError(f"fps must be positive, got {fps!r}")
    cs_per_frame = 100.0 / fps
    out: List[int] = []
    prev_cum = 0
    for i in range(1, n_frames + 1):
        cum = round(i * cs_per_frame)
        out.append(max(1, cum - prev_cum))
        prev_cum = cum
    return out


# ── capture: pause -> arm -> step -> grab ────────────────────────────────


def _call(client: McpClient, tool: str, **kwargs: Any) -> dict:
    """Call one MCP tool and raise `CaptureError` unless it clearly
    succeeded. Two failure shapes exist server-side: most tools answer
    `{"ok": false, "error": ...}`, but the write gate short-circuits to a
    bare `{"error": ...}` with no `"ok"` key at all — an absent `"ok"` is
    treated as failure too, so a forgotten `enable_writes` is never read as
    success."""
    r = client.call(tool, **kwargs)
    failed = isinstance(r, dict) and (
        r.get("ok") is False or ("error" in r and "ok" not in r)
    )
    if failed:
        raise CaptureError(f"{tool} failed: {r.get('error', r)}")
    return r


def _step(client: McpClient) -> Optional[int]:
    """Advance exactly one frame and CONFIRM it landed. `step` is
    synchronous: the response only comes back once the emulated frame is
    entirely finished, and a response that does not say `landed: true` is a
    hard failure here — never a retry."""
    r = _call(client, "step")
    if not r.get("landed", False):
        raise CaptureError(f"step() did not land: {r}")
    return r.get("frame_count")


def _grab(client: McpClient, out_dir: Path, index: int) -> Path:
    png = client.read_resource("app://screen")
    path = out_dir / f"frame_{index:04d}.png"
    path.write_bytes(png)
    return path


def capture_slot(
    client: McpClient,
    *,
    slot_name: str,
    state_path: str,
    out_dir: Union[str, Path],
) -> List[str]:
    """Replay `slot_name` (bare name, no `.slot.json`) from `state_path`,
    frame by frame, saving each landed frame's `app://screen` PNG under
    `out_dir`. Returns the written PNG paths in playback order.

    The returned list is `1 + frames + 1` long: a PRE-ROLL bookend (grabbed
    right after the atomic load, before arming), the slot's own `frames`
    frames, and a POST-ROLL bookend (one extra `step()` past the slot's last
    frame, where the input release lands and the playback clears).

    **Caveat on the pre-roll bookend, measured live**: `app://screen` reports
    the LAST frame the core actually rendered — loading a save state does
    not itself trigger a re-render, only the next `step`/`run_frame` does.
    On the FIRST capture of a freshly-launched headless instance (nothing
    rendered yet), the pre-roll bookend is a near-black boot frame, not the
    loaded arena. On any later capture in the same session it correctly
    shows the previous capture's last rendered frame. A caller that needs a
    guaranteed look at the loaded arena itself should treat the first REAL
    slot frame (index 1) as the "before" reference, not index 0.

    Any playback already armed/active is stopped first (defensively;
    `play_inputs` refuses to arm over one) and the playback is stopped again
    at the end regardless of how the loop exits, so a failed capture never
    leaves a stale playback driving the next caller's ports.
    """
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    _call(client, "enable_writes")

    load = _call(client, "load_state", path=str(state_path), pause_after=True)
    if not load.get("paused"):
        raise CaptureError(
            f"load_state({state_path!r}, pause_after=True) reported ok but "
            f"not paused=true (got {load!r}) — the atomic load-and-pause "
            "guarantee did not hold; nothing captured from here is "
            "trustworthy."
        )

    try:
        client.call("play_inputs", action="stop")
    except Exception:
        pass  # no playback was active — the normal case, not an error

    paths: List[Path] = []
    idx = 0
    try:
        paths.append(_grab(client, out_dir, idx)); idx += 1  # pre-roll bookend

        start = _call(
            client, "play_inputs",
            action="start", name=slot_name, port="both", trigger="manual",
        )
        n_frames = int(start.get("frames", 0))
        if n_frames <= 0:
            raise CaptureError(
                f"play_inputs(start, name={slot_name!r}) reported {n_frames} "
                "frames — the slot is empty or was not found."
            )

        for _ in range(n_frames):
            _step(client)
            paths.append(_grab(client, out_dir, idx)); idx += 1

        # The extra step: the slot's final-frame release lands here.
        _step(client)
        paths.append(_grab(client, out_dir, idx)); idx += 1
    finally:
        try:
            client.call("play_inputs", action="stop")
        except Exception:
            pass

    return [str(p) for p in paths]


# ── assembly ──────────────────────────────────────────────────────────────


def _draw_caption_banner(
    img: Image.Image, texts: Sequence[str], *, position: str = "bottom"
) -> Image.Image:
    """Draw a solid-backed, readable banner listing `texts` (one per line)
    at the top or bottom of `img`. Font size scales with the (already
    upscaled) frame width so a caption stays legible regardless of `scale`."""
    if not texts:
        return img
    draw = ImageDraw.Draw(img)
    size = max(12, img.width // 22)
    try:
        font = ImageFont.load_default(size=size)
    except TypeError:  # Pillow < 10.1: load_default() takes no size arg
        font = ImageFont.load_default()

    pad = max(3, size // 4)
    text = "\n".join(texts)
    bbox = draw.multiline_textbbox((0, 0), text, font=font)
    text_h = bbox[3] - bbox[1]
    box_h = min(img.height, text_h + 2 * pad)
    y0 = img.height - box_h if position == "bottom" else 0

    draw.rectangle([0, y0, img.width, y0 + box_h], fill=(0, 0, 0))
    draw.multiline_text(
        (pad, y0 + pad - bbox[1]), text, font=font, fill=(255, 255, 255), align="left"
    )
    return img


def assemble_gif(
    png_paths: Sequence[str],
    out_path: Union[str, Path],
    *,
    fps_native: float = NATIVE_FPS_MK2,
    scale: int = 2,
    captions: Optional[Sequence[Union[Caption, Dict[str, Any]]]] = None,
    caption_position: str = "bottom",
) -> str:
    """Assemble `png_paths` (in playback order — `capture_slot`'s return
    value, or any equivalent list) into a GIF at `out_path`.

    `scale` is an integer nearest-neighbor upscale (`Image.NEAREST` — the
    correct resampling for pixel art; anything smoother would blur the exact
    per-pixel frame data a caption sits beside). Captions are drawn AFTER
    upscaling so a fixed font size stays legible independent of `scale`.

    `captions` items are `{start_frame, end_frame, text}` (or `Caption`
    instances), indices into `png_paths` itself (0-based, inclusive both
    ends) — see `Caption`'s docstring for the bookend-offset caveat.

    Duration comes from `frame_durations_cs`, converted to the milliseconds
    Pillow's GIF writer expects (`cs * 10` — already centisecond-granular,
    so no further rounding loss versus what the GIF format can store).
    """
    if not png_paths:
        raise ValueError("assemble_gif: no frames given")
    if scale < 1:
        raise ValueError(f"scale must be >= 1, got {scale!r}")
    caps = _coerce_captions(captions)

    frames: List[Image.Image] = []
    for i, p in enumerate(png_paths):
        img = Image.open(p).convert("RGB")
        if scale != 1:
            img = img.resize((img.width * scale, img.height * scale), Image.NEAREST)
        texts = captions_for_frame(caps, i)
        if texts:
            img = _draw_caption_banner(img, texts, position=caption_position)
        frames.append(img)

    durations_ms = [cs * 10 for cs in frame_durations_cs(len(frames), fps=fps_native)]

    out_path = str(out_path)
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(
        out_path,
        save_all=True,
        append_images=frames[1:],
        duration=durations_ms,
        loop=0,
        optimize=False,
    )
    return out_path


# ── CLI ───────────────────────────────────────────────────────────────────


def _load_captions_arg(path: Optional[str]) -> Optional[List[dict]]:
    if not path:
        return None
    return json.loads(Path(path).read_text())


def main() -> None:  # pragma: no cover - thin CLI wrapper over tested code
    """`python -m shadow_train.capture <capture|gif|demo> ...`

    Never point `--url` at port 4025 (CLAUDE.md: the user's live session) —
    use a headless instance on a distinct port (this module's own smoke test
    uses 4027).
    """
    ap = argparse.ArgumentParser(description=main.__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    cap = sub.add_parser("capture", help="replay a slot, write per-frame PNGs")
    cap.add_argument("--url", default="http://127.0.0.1:4027/mcp")
    cap.add_argument("--slot", required=True, help="bare slot name (no .slot.json)")
    cap.add_argument("--state", required=True, help="arena .state path")
    cap.add_argument("--out-dir", required=True)

    gif = sub.add_parser("gif", help="assemble a PNG directory into a GIF")
    gif.add_argument("--frames-dir", required=True)
    gif.add_argument("--out", required=True)
    gif.add_argument("--fps", type=float, default=NATIVE_FPS_MK2)
    gif.add_argument("--scale", type=int, default=2)
    gif.add_argument("--captions", default=None, help="JSON file: list of {start_frame,end_frame,text}")

    demo = sub.add_parser("demo", help="capture + assemble in one shot")
    demo.add_argument("--url", default="http://127.0.0.1:4027/mcp")
    demo.add_argument("--slot", required=True)
    demo.add_argument("--state", required=True)
    demo.add_argument("--out", required=True)
    demo.add_argument("--work-dir", default=None, help="default: a fresh temp dir")
    demo.add_argument("--fps", type=float, default=NATIVE_FPS_MK2)
    demo.add_argument("--scale", type=int, default=2)
    demo.add_argument("--captions", default=None, help="JSON file: list of {start_frame,end_frame,text}")

    args = ap.parse_args()

    if args.cmd == "capture":
        client = McpClient(args.url)
        paths = capture_slot(client, slot_name=args.slot, state_path=args.state, out_dir=args.out_dir)
        print(f"wrote {len(paths)} frames to {args.out_dir}")
    elif args.cmd == "gif":
        paths = sorted(str(p) for p in Path(args.frames_dir).glob("frame_*.png"))
        out = assemble_gif(
            paths, args.out, fps_native=args.fps, scale=args.scale,
            captions=_load_captions_arg(args.captions),
        )
        print(f"wrote {out} ({len(paths)} frames)")
    elif args.cmd == "demo":
        work_dir = args.work_dir or tempfile.mkdtemp(prefix="capture-")
        client = McpClient(args.url)
        paths = capture_slot(client, slot_name=args.slot, state_path=args.state, out_dir=work_dir)
        out = assemble_gif(
            paths, args.out, fps_native=args.fps, scale=args.scale,
            captions=_load_captions_arg(args.captions),
        )
        print(f"wrote {out} ({len(paths)} frames, work dir {work_dir})")


if __name__ == "__main__":  # pragma: no cover
    main()
