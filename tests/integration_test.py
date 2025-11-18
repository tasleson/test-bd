#!/usr/bin/env python3
import argparse
import json
import random
import subprocess
import tempfile
import sys
from pathlib import Path


def rand_percent_triplet():
    """
    Generate (fill, dup, rand) such that all are >= 0 and sum to 100.
    Uniform over partitions via two random cut points.
    """
    a = random.randint(0, 100)
    b = random.randint(0, 100)
    if b < a:
        a, b = b, a
    fill = a
    dup = b - a
    rnd = 100 - b
    return fill, dup, rnd


def run_cmd(cmd, **kwargs):
    """Run subprocess, capture stdout/stderr, do not raise on error."""
    print(f"+ {' '.join(cmd)}")
    result = subprocess.run(cmd, text=True, capture_output=True, **kwargs)
    return result


def main():
    parser = argparse.ArgumentParser(description="Integration test loop for test-bd / test-bd-verify")
    parser.add_argument(
        "--test-bd",
        default="/home/tasleson/bin/test-bd",
        help="Path to test-bd binary (default: test-bd)",
    )
    parser.add_argument(
        "--verify",
        default="/home/tasleson/bin/test-bd-verify",
        help="Path to test-bd-verify binary (default: test-bd-verify)",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=100,
        help="Number of test iterations to run (default: 100)",
    )
    parser.add_argument(
        "--id",
        type=int,
        default=0,
        help="Block device ID to use (default: 0)",
    )
    parser.add_argument(
        "--stop-on-fail",
        action="store_true",
        help="Stop the loop on first failure (add/verify) instead of continuing",
    )
    parser.add_argument(
        "--min-size-mib",
        type=int,
        default=1,
        help="Minimum size in MiB (default: 1)",
    )
    parser.add_argument(
        "--max-size-mib",
        type=int,
        default=10,
        help="Maximum size in MiB (default: 10)",
    )
    parser.add_argument(
        "--min-segments",
        type=int,
        default=1,
        help="Minimum number of segments (default: 1)",
    )
    parser.add_argument(
        "--max-segments",
        type=int,
        default=1024,
        help="Maximum number of segments (default: 1024)",
    )
    args = parser.parse_args()

    test_bd = Path(args.test_bd)
    verify = Path(args.verify)

    if not test_bd.is_file():
        print(f"error: test-bd not found at {test_bd}", file=sys.stderr)
        sys.exit(1)
    if not verify.is_file():
        print(f"error: test-bd-verify not found at {verify}", file=sys.stderr)
        sys.exit(1)

    for it in range(1, args.iterations + 1):
        print(f"\n=== Iteration {it}/{args.iterations} ===")

        # Random parameters
        size_mib = random.randint(args.min_size_mib, args.max_size_mib)
        size_arg = f"{size_mib} MiB"

        segments = random.randint(args.min_segments, args.max_segments)
        fill_pct, dup_pct, rnd_pct = rand_percent_triplet()

        print(
            f"Parameters: size={size_arg}, segments={segments}, "
            f"fill={fill_pct}, dup={dup_pct}, random={rnd_pct}"
        )

        manifest_path = None
        dev_path = None

        # 1. Add block device
        add_cmd = [
            str(test_bd),
            "add",
            "--id",
            str(args.id),
            "--size",
            size_arg,
            "--fill",
            str(fill_pct),
            "--duplicate",
            str(dup_pct),
            "--random",
            str(rnd_pct),
            "--segments",
            str(segments),
            "-J",  # JSON output
        ]

        add_res = run_cmd(add_cmd)
        if add_res.returncode != 0:
            print(f"ERROR: test-bd add failed (rc={add_res.returncode})")
            print("stdout:\n", add_res.stdout)
            print("stderr:\n", add_res.stderr, file=sys.stderr)
            if args.stop_on_fail:
                sys.exit(add_res.returncode)
            else:
                continue

        # 2. Parse JSON from add output
        try:
            info = json.loads(add_res.stdout)
        except json.JSONDecodeError as e:
            print("ERROR: failed to parse JSON from test-bd add output", file=sys.stderr)
            print("stdout was:\n", add_res.stdout)
            if args.stop_on_fail:
                sys.exit(1)
            else:
                continue



        dev_path = f"/dev/ublkb{info.get("device_id")}"


        tmp = tempfile.NamedTemporaryFile("w", delete=False, suffix=".json")
        manifest_path = tmp.name
        tmp.write(add_res.stdout)
        tmp.close()


        # The block device might not be readable as udev might be holding on to is
        import time
        time.sleep(1)


        # 3. Run verifier
        verify_cmd = [
            str(verify),
            dev_path,
            manifest_path,
        ]
        verify_res = run_cmd(verify_cmd)
        if verify_res.returncode != 0:
            print(f"ERROR: test-bd-verify failed (rc={verify_res.returncode})")
            print("stdout:\n", verify_res.stdout)
            print("stderr:\n", verify_res.stderr, file=sys.stderr)
            if args.stop_on_fail:
                # Best-effort cleanup before exiting
                del_cmd = [str(test_bd), "del", "--id", str(args.id)]
                run_cmd(del_cmd)
                sys.exit(verify_res.returncode)
        else:
            print("verify: success")

        # 4. Remove block device (best-effort)
        del_cmd = [
            str(test_bd),
            "del",
            "--id",
            str(args.id),
        ]
        del_res = run_cmd(del_cmd)
        if del_res.returncode != 0:
            print(f"WARNING: test-bd del failed (rc={del_res.returncode})", file=sys.stderr)
            print("stdout:\n", del_res.stdout)
            print("stderr:\n", del_res.stderr, file=sys.stderr)
            if args.stop_on_fail:
                sys.exit(del_res.returncode)

    print("\nAll iterations complete.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
