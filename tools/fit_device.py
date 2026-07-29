#!/usr/bin/env python3
"""Fit a diode's (Is, n) from I-V points — the two numbers every clipper needs.

Tone Revolution phase 08 (PRD 035), the reshaped §2.2 of the phase plan.

WHY THIS AND NOT SPICE
----------------------
The plan asked for a `sim/<pedal>/fit.py` flow: draw the circuit in LTSpice,
run a transient, fit the device parameters against the simulated curve. That
route needs a simulator this project does not have and does not want to
depend on — Phase 02 already replaced its ngspice fixtures with an independent
nodal-analysis oracle for the same reason, and Phase 08 now carries a full
reference solver (`lh_dsp::testutil::netlist`) that does the circuit-level job
better, in CI, with no external binary.

What remains genuinely useful is the *device*-level fit, and that never needed
a simulator: the Shockley equation is one exponential, and fitting it to a
handful of datasheet points is a two-parameter least-squares problem.

    i = Is * (exp(v / (n * Vt)) - 1)

ADR 033 is why both parameters are fitted rather than just `Is`: the knee is
`v ~ n*Vt*ln(i/Is)`, so `Is` and `n` are not separable. Germanium is not
"silicon with a different Is" — it is high Is *and* near-unity n, and that
pairing is what puts its knee at 0.3 V instead of 0.6 V. A menu carrying `Is`
alone against a shared `n` can make germanium clip *higher* than silicon,
which is exactly the defect ADR 033 records in the reference implementation.

USAGE
-----
    # datasheet points, as volts,amps pairs
    python3 tools/fit_device.py --points 0.5,1e-4 0.6,1e-3 0.7,1e-2

    # or a CSV with a `v,i` column pair (header optional)
    python3 tools/fit_device.py --csv 1n4148.csv

    # sanity-check the fit by printing the residual table
    python3 tools/fit_device.py --csv 1n4148.csv --show-residuals

    # fit a *pair* rather than a single device: i = 2*Is*sinh(v/(n*Vt)),
    # which is what a WDF `DiodePair` root actually evaluates
    python3 tools/fit_device.py --pair --points 0.5,1e-4 0.6,1e-3

Output is the Rust line to paste into a pedal's diode table.

Requires numpy and scipy. Nothing here runs at audio time or ships in the
binary; it is a bench tool.
"""

from __future__ import annotations

import argparse
import csv
import sys

import numpy as np
from scipy.optimize import least_squares

# Thermal voltage at room temperature, the value every model in this project
# uses. Fitting at a different junction temperature means changing this, and
# the fitted `n` will absorb the difference if you do not.
VT = 0.02585


def parse_points(pairs: list[str]) -> np.ndarray:
    out = []
    for p in pairs:
        try:
            v, i = p.split(",")
            out.append((float(v), float(i)))
        except ValueError:
            sys.exit(f"bad point {p!r}: expected volts,amps (e.g. 0.6,1e-3)")
    return np.array(out)


def parse_csv(path: str) -> np.ndarray:
    out = []
    with open(path, newline="") as fh:
        for row in csv.reader(fh):
            if len(row) < 2:
                continue
            try:
                out.append((float(row[0]), float(row[1])))
            except ValueError:
                continue  # header or blank line
    if not out:
        sys.exit(f"{path}: no numeric v,i rows found")
    return np.array(out)


def model(params: np.ndarray, v: np.ndarray, pair: bool) -> np.ndarray:
    """Current at `v` for `params = [log10(Is), n]`.

    `Is` is fitted in log space because it spans a dozen decades across the
    devices this project cares about (1e-16 for an LED, 1e-7 for germanium),
    and a linear parameter would make the solver crawl at the small end.
    """
    is_, n = 10.0 ** params[0], params[1]
    u = np.clip(v / (n * VT), -80.0, 80.0)
    if pair:
        # An antiparallel pair, which is what a `DiodePair` root evaluates.
        return 2.0 * is_ * np.sinh(u)
    return is_ * (np.exp(u) - 1.0)


def residuals(params: np.ndarray, v: np.ndarray, i: np.ndarray, pair: bool) -> np.ndarray:
    """Residuals in **log current**, not in amps.

    Fitting in amps would weight one point at 10 mA more than every point
    below 1 mA put together, and the low-current end is precisely where a
    guitar clipper spends its time — the knee, not the hard-on region.
    """
    got = model(params, v, pair)
    return np.log10(np.maximum(np.abs(got), 1e-30)) - np.log10(np.abs(i))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--points", nargs="+", metavar="V,I", help="volts,amps pairs")
    ap.add_argument("--csv", metavar="PATH", help="CSV with v,i columns")
    ap.add_argument(
        "--pair",
        action="store_true",
        help="fit an antiparallel pair (2*Is*sinh) instead of one device",
    )
    ap.add_argument("--name", default="MY_DIODE", help="constant name for the Rust output")
    ap.add_argument("--show-residuals", action="store_true")
    args = ap.parse_args()

    if bool(args.points) == bool(args.csv):
        sys.exit("give exactly one of --points or --csv")

    data = parse_points(args.points) if args.points else parse_csv(args.csv)
    v, i = data[:, 0], data[:, 1]
    if len(v) < 2:
        sys.exit("need at least two points to fit two parameters")

    # Seed from the two extreme points, which is enough to land in the basin:
    # the slope of ln(i) against v is 1/(n*Vt).
    lo, hi = np.argmin(v), np.argmax(v)
    slope = (np.log(abs(i[hi])) - np.log(abs(i[lo]))) / max(v[hi] - v[lo], 1e-9)
    n0 = float(np.clip(1.0 / (slope * VT), 1.0, 4.0)) if slope > 0 else 1.8
    is0 = float(np.log10(max(abs(i[lo]) * np.exp(-v[lo] / (n0 * VT)), 1e-30)))

    fit = least_squares(
        residuals,
        x0=[is0, n0],
        args=(v, i, args.pair),
        bounds=([-30.0, 0.8], [0.0, 6.0]),
    )
    is_, n = 10.0 ** fit.x[0], fit.x[1]
    rms = float(np.sqrt(np.mean(fit.fun**2)))

    print(f"Is = {is_:.4g} A")
    print(f"n  = {n:.4g}")
    print(f"knee at 1 mA: {n * VT * np.log(1e-3 / is_):.3f} V")
    print(f"residual: {rms:.4f} decades RMS over {len(v)} points")
    if rms > 0.1:
        print("  ^ that is a poor fit; check the points are one device's forward curve")

    if args.show_residuals:
        print()
        print(f"{'V':>8} {'I meas':>12} {'I fit':>12} {'ratio':>8}")
        got = model(fit.x, v, args.pair)
        for vv, im, ig in zip(v, i, got):
            print(f"{vv:8.3f} {im:12.4e} {ig:12.4e} {ig / im:8.3f}")

    kind = "pair-level" if args.pair else "single-device"
    print()
    print("// paste into the pedal's diode table (ADR 033: (Is, n), never Is alone)")
    print(f"/// {args.name} — {kind} fit, {rms:.3f} decades RMS over {len(v)} points.")
    print(f"static {args.name}_MODEL: (f32, f32) = ({is_:.4g}, {n:.4g});")


if __name__ == "__main__":
    main()
