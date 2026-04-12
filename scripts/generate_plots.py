#!/usr/bin/env python3
"""Generate benchmark plots for the Eruditio README.

Outputs:
  media/conversion_time.png  — Eruditio vs Calibre bar chart (log scale)
  media/speedup.png          — Speedup factor bar chart
  media/intrinsics.png       — SIMD intrinsics throughput (scalar vs AVX512)
"""

import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.ticker as ticker
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
MEDIA = ROOT / "media"
CRITERION = ROOT / "target" / "criterion"

# ── Calibre comparison data (from real-world median-of-31 measurements) ───────

CONVERSIONS = [
    ("EPUB->MOBI (135 KB)",   8.1, 1079),
    ("EPUB->MOBI (715 KB)",  15.0, 3917),
    ("EPUB->TXT (135 KB)",    4.6,  497),
    ("EPUB->TXT (715 KB)",   16.0,  951),
    ("EPUB->FB2 (135 KB)",    7.1,  604),
    ("EPUB->FB2 (715 KB)",    9.1, 1105),
    ("HTML->EPUB (146 KB)",   2.9,  513),
    ("HTML->EPUB (941 KB)",   8.7, 1957),
    ("FB2->EPUB (1.2 MB)",    7.8,  569),
]

# ── Plot 1: Conversion time (horizontal bar, log scale) ──────────────────────

def plot_conversion_time():
    labels = [c[0] for c in CONVERSIONS]
    eruditio = [c[1] for c in CONVERSIONS]
    calibre = [c[2] for c in CONVERSIONS]

    fig, ax = plt.subplots(figsize=(12, 5.5))
    y = np.arange(len(labels))
    h = 0.35

    bars_c = ax.barh(y - h / 2, calibre, h, label="Calibre 9.6", color="#d94f4f",
                     edgecolor="white", linewidth=0.5)
    bars_e = ax.barh(y + h / 2, eruditio, h, label="Eruditio", color="#4a86c8",
                     edgecolor="white", linewidth=0.5)

    ax.set_xscale("log")
    ax.set_xlabel("Time (ms, log scale)", fontsize=11)
    ax.set_yticks(y)
    ax.set_yticklabels(labels, fontsize=10)
    ax.invert_yaxis()
    ax.set_title("Ebook Conversion Time: Eruditio vs Calibre", fontsize=14, fontweight="bold")
    ax.legend(loc="lower right", fontsize=10)
    ax.xaxis.set_major_formatter(ticker.ScalarFormatter())

    for bar, val in zip(bars_e, eruditio):
        ax.text(bar.get_width() * 1.15, bar.get_y() + bar.get_height() / 2,
                f"{val} ms", va="center", fontsize=8.5, color="#2a5a9c", fontweight="bold")
    for bar, val in zip(bars_c, calibre):
        ax.text(bar.get_width() * 1.15, bar.get_y() + bar.get_height() / 2,
                f"{val} ms", va="center", fontsize=8.5, color="#a03030", fontweight="bold")

    fig.tight_layout()
    fig.savefig(MEDIA / "conversion_time.png", dpi=150)
    plt.close(fig)
    print(f"  wrote {MEDIA / 'conversion_time.png'}")


# ── Plot 2: Speedup factor ───────────────────────────────────────────────────

def plot_speedup():
    labels = [c[0] for c in CONVERSIONS]
    speedups = [round(c[2] / c[1]) for c in CONVERSIONS]
    avg = sum(speedups) / len(speedups)

    fig, ax = plt.subplots(figsize=(12, 5))
    y = np.arange(len(labels))

    # Gradient-like coloring: darker blue for higher speedup
    max_s = max(speedups)
    colors = [plt.cm.Blues(0.35 + 0.55 * s / max_s) for s in speedups]

    bars = ax.barh(y, speedups, 0.55, color=colors, edgecolor="white", linewidth=0.5)

    ax.axvline(avg, color="#e09520", linestyle="--", linewidth=1.5, zorder=0)
    ax.text(avg + 2, len(labels) - 0.5, f"avg: {avg:.0f}x",
            color="#c07000", fontsize=10, fontweight="bold")

    for bar, s in zip(bars, speedups):
        ax.text(bar.get_width() + 2, bar.get_y() + bar.get_height() / 2,
                f"{s}x", va="center", fontsize=10, color="#1a3a6a", fontweight="bold")

    ax.set_xlabel("Speedup (x faster than Calibre)", fontsize=11)
    ax.set_yticks(y)
    ax.set_yticklabels(labels, fontsize=10)
    ax.invert_yaxis()
    ax.set_title("Eruditio Speedup over Calibre", fontsize=14, fontweight="bold")
    ax.set_xlim(0, max(speedups) + 30)

    fig.tight_layout()
    fig.savefig(MEDIA / "speedup.png", dpi=150)
    plt.close(fig)
    print(f"  wrote {MEDIA / 'speedup.png'}")


# ── Plot 3: SIMD intrinsics throughput ────────────────────────────────────────

def read_criterion(bench_name):
    """Read the mean point estimate (ns) from a Criterion benchmark."""
    path = CRITERION / bench_name / "new" / "estimates.json"
    if not path.exists():
        return None
    with open(path) as f:
        d = json.load(f)
    return d["mean"]["point_estimate"]


def plot_intrinsics():
    # (label, data_size_bytes, scalar_bench_or_None, simd_bench)
    intrinsics = [
        ("is_ascii\n(1 KB)",          1024,  "is_ascii_scalar_1k",           "is_ascii_simd_1k"),
        ("skip_ws\n(1 KB)",           1024,  "skip_ws_scalar_1k",            "skip_ws_simd_1k"),
        ("short_pat\n(10 KB miss)",   10000, "short_pat_scalar_2b_miss_10k", "short_pat_simd_2b_miss_10k"),
        ("cp1252\n(10 KB ASCII)",     10000, None,                           "cp1252_decode_10k_ascii"),
        ("byte_scan\n(10 KB clean)",  10000, None,                           "byte_scan_clean_10k"),
        ("case_fold\n(1 KB)",         1024,  None,                           "case_fold_eq_1k"),
        ("find_ci\n(50 KB miss)",     50000, None,                           "find_ci_missing_in_50k_html"),
    ]

    labels, scalar_tp, simd_tp = [], [], []
    for label, size, scalar_name, simd_name in intrinsics:
        m_ns = read_criterion(simd_name)
        if m_ns is None:
            continue
        s_ns = read_criterion(scalar_name) if scalar_name else None
        labels.append(label)
        scalar_tp.append(size / s_ns if s_ns else 0)
        simd_tp.append(size / m_ns)

    if not labels:
        print("  skipped intrinsics.png (no criterion data)")
        return

    fig, ax = plt.subplots(figsize=(11, 5))
    x = np.arange(len(labels))
    w = 0.35

    # Scalar bars (only where we have data)
    bars_s = ax.bar(x - w / 2, scalar_tp, w, label="Scalar", color="#b0b0b0",
                    edgecolor="white", linewidth=0.5)
    bars_m = ax.bar(x + w / 2, simd_tp, w, label="AVX-512", color="#2e7d32",
                    edgecolor="white", linewidth=0.5)

    max_val = max(max(scalar_tp), max(simd_tp))
    for bar, val in zip(bars_s, scalar_tp):
        if val > 0:
            ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height() + max_val * 0.01,
                    f"{val:.1f}", ha="center", fontsize=8.5, color="#555")
    for bar, val in zip(bars_m, simd_tp):
        ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height() + max_val * 0.01,
                f"{val:.1f}", ha="center", fontsize=8.5, color="#1b5e20", fontweight="bold")

    ax.set_ylabel("Throughput (GB/s)", fontsize=11)
    ax.set_xticks(x)
    ax.set_xticklabels(labels, fontsize=9)
    ax.set_title("SIMD Intrinsics Throughput (AVX-512 vs Scalar)", fontsize=14, fontweight="bold")
    ax.legend(loc="upper right", fontsize=10)
    ax.set_ylim(0, max_val * 1.2)

    fig.tight_layout()
    fig.savefig(MEDIA / "intrinsics.png", dpi=150)
    plt.close(fig)
    print(f"  wrote {MEDIA / 'intrinsics.png'}")


if __name__ == "__main__":
    MEDIA.mkdir(exist_ok=True)
    print("Generating benchmark plots...")
    plot_conversion_time()
    plot_speedup()
    plot_intrinsics()
    print("Done.")
