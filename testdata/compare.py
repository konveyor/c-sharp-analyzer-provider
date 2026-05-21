# /// script
# requires-python = ">=3.11"
# ///
"""Compare Rust vs C# analyzer provider results on test datasets."""

import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TESTDATA = REPO_ROOT / "testdata"
RESULTS = TESTDATA / "results"
RUST_PORT = 9000
CSHARP_PORT = 9876

DATASETS = {
    "nerd-dinner": TESTDATA / "nerd-dinner" / "mvc4",
    "net8-sample": TESTDATA / "net8-sample",
}

RUST_PATTERNS = {
    "nerd-dinner": [
        "System.Web.Mvc.*",
        "System.Web.Http.*",
        "System.Data.Entity.*",
        "NerdDinner..*",
        "System.Web.Mvc.Controller",
    ],
    "net8-sample": [
        "System..*",
        "Net8Sample..*",
        "System.Console.WriteLine",
    ],
}

CSHARP_PATTERNS = {
    "nerd-dinner": [
        r"System\.Web\.Mvc.*",
        r"System\.Web\.Http.*",
        r"System\.Data\.Entity.*",
        r"NerdDinner\..*",
        "System.Web.Mvc.Controller",
    ],
    "net8-sample": [
        r"System\..*",
        r"Net8Sample\..*",
        "System.Console.WriteLine",
    ],
}

servers: list[subprocess.Popen] = []


def cleanup():
    for proc in servers:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        except (ProcessLookupError, OSError):
            pass
    servers.clear()
    for port in (RUST_PORT, CSHARP_PORT):
        kill_port(port)


def kill_port(port: int):
    try:
        out = subprocess.check_output(
            ["lsof", "-ti", f":{port}"], stderr=subprocess.DEVNULL, text=True
        )
        for pid in out.strip().split("\n"):
            if pid:
                os.kill(int(pid), signal.SIGTERM)
    except (subprocess.CalledProcessError, OSError):
        pass


def wait_for_port(port: int, timeout: int = 120) -> bool:
    print(f"  Waiting for port {port}...", end="", flush=True)
    for i in range(timeout):
        try:
            with socket.create_connection(("localhost", port), timeout=1):
                print(f" ready ({i+1}s)")
                return True
        except OSError:
            time.sleep(1)
    print(f" TIMEOUT after {timeout}s")
    return False


def start_server(cmd: list[str], port: int, label: str) -> subprocess.Popen:
    kill_port(port)
    time.sleep(1)
    print(f"  Starting {label} on port {port}...")
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        preexec_fn=os.setsid,
    )
    servers.append(proc)
    if not wait_for_port(port):
        print(f"  ERROR: {label} failed to start")
        sys.exit(1)
    return proc


def grpcurl(port: int, data: dict, method: str, max_time: int = 300) -> dict | None:
    cmd = [
        "grpcurl",
        "-max-msg-sz", "10485760",
        "-max-time", str(max_time),
        "-plaintext",
        "-d", json.dumps(data),
        f"localhost:{port}",
        f"provider.ProviderService/{method}",
    ]
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=max_time + 30
        )
        if result.returncode != 0:
            print(f"    grpcurl error: {result.stderr.strip()[:200]}")
            return None
        return json.loads(result.stdout)
    except subprocess.TimeoutExpired:
        print(f"    grpcurl timed out after {max_time}s")
        return None
    except json.JSONDecodeError as e:
        print(f"    Invalid JSON response: {e}")
        return None


def run_init_rust(port: int, location: str) -> dict | None:
    ilspy = shutil.which("ilspycmd")
    paket = shutil.which("paket")
    if not ilspy or not paket:
        print(f"    WARNING: ilspycmd={ilspy}, paket={paket}")
    data = {
        "analysisMode": "source-only",
        "location": location,
    }
    if ilspy and paket:
        data["providerSpecificConfig"] = {
            "ilspy_cmd": ilspy,
            "paket_cmd": paket,
        }
    return grpcurl(port, data, "Init", max_time=600)


def run_init_csharp(port: int, location: str) -> dict | None:
    return grpcurl(port, {"location": location}, "Init", max_time=600)


def run_evaluate(port: int, pattern: str) -> dict | None:
    condition = json.dumps({"referenced": {"pattern": pattern}})
    data = {"cap": "referenced", "id": "1", "conditionInfo": condition}
    return grpcurl(port, data, "Evaluate")


def safe_name(pattern: str) -> str:
    return pattern.replace("\\", "_").replace(".", "_").replace("*", "_")


def normalize(response: dict | None) -> list[dict]:
    if not response:
        return []
    incidents = (
        response.get("response", {}).get("incidentContexts", [])
    )
    return sorted(incidents, key=lambda r: (r.get("fileURI", ""), r.get("LineNumber", "0")))


def diff_results(
    rust_resp: dict | None, csharp_resp: dict | None, pattern: str
) -> dict:
    rust_items = normalize(rust_resp)
    csharp_items = normalize(csharp_resp)

    rust_keys = {(r["fileURI"], r.get("LineNumber", "0")) for r in rust_items}
    csharp_keys = {(r["fileURI"], r.get("LineNumber", "0")) for r in csharp_items}

    rust_only = [
        r for r in rust_items
        if (r["fileURI"], r.get("LineNumber", "0")) not in csharp_keys
    ]
    csharp_only = [
        r for r in csharp_items
        if (r["fileURI"], r.get("LineNumber", "0")) not in rust_keys
    ]

    return {
        "query": pattern,
        "rust_count": len(rust_items),
        "csharp_count": len(csharp_items),
        "rust_only_count": len(rust_only),
        "csharp_only_count": len(csharp_only),
        "rust_only": rust_only,
        "csharp_only": csharp_only,
    }


def build():
    print("=== Building Rust provider ===")
    r = subprocess.run(
        ["cargo", "build"], cwd=REPO_ROOT, capture_output=True, text=True
    )
    if r.returncode != 0:
        print(f"  Rust build failed:\n{r.stderr[-500:]}")
        sys.exit(1)
    print("  OK")

    print("=== Building C# provider ===")
    r = subprocess.run(
        ["dotnet", "build", "CSharpProvider/CSharpProvider.csproj"],
        cwd=REPO_ROOT, capture_output=True, text=True,
    )
    if r.returncode != 0:
        print(f"  C# build failed:\n{r.stderr[-500:]}")
        sys.exit(1)
    print("  OK")


def main():
    signal.signal(signal.SIGINT, lambda *_: (cleanup(), sys.exit(1)))
    signal.signal(signal.SIGTERM, lambda *_: (cleanup(), sys.exit(1)))

    build()

    for d in ("rust", "csharp", "diff"):
        (RESULTS / d).mkdir(parents=True, exist_ok=True)

    for dataset, location in DATASETS.items():
        rust_patterns = RUST_PATTERNS[dataset]
        csharp_patterns = CSHARP_PATTERNS[dataset]
        print(f"\n{'='*50}")
        print(f"  Dataset: {dataset}")
        print(f"  Location: {location}")
        print(f"{'='*50}")

        # ── Rust provider ──
        print(f"\n--- Rust provider (port {RUST_PORT}) ---")
        start_server(
            ["cargo", "run", "--", "--port", str(RUST_PORT), "--name", "c-sharp"],
            RUST_PORT, "Rust provider",
        )

        print("  Init...")
        init_resp = run_init_rust(RUST_PORT, str(location))
        if init_resp and init_resp.get("successful"):
            print(f"  Init OK")
        else:
            print(f"  Init FAILED: {init_resp}")

        rust_results = {}
        for pattern in rust_patterns:
            print(f"  Evaluate: {pattern}")
            rust_results[pattern] = run_evaluate(RUST_PORT, pattern)
            sn = safe_name(pattern)
            out = RESULTS / "rust" / f"{dataset}_{sn}.json"
            out.write_text(json.dumps(rust_results[pattern], indent=2) if rust_results[pattern] else "{}")

        cleanup()

        # ── C# provider ──
        print(f"\n--- C# provider (port {CSHARP_PORT}) ---")
        start_server(
            ["dotnet", "run", "--project", "CSharpProvider", "--", "--port", str(CSHARP_PORT)],
            CSHARP_PORT, "C# provider",
        )

        print("  Init...")
        init_resp = run_init_csharp(CSHARP_PORT, str(location))
        if init_resp and init_resp.get("successful"):
            print(f"  Init OK")
        else:
            print(f"  Init FAILED: {init_resp}")

        csharp_results = {}
        for pattern in csharp_patterns:
            print(f"  Evaluate: {pattern}")
            csharp_results[pattern] = run_evaluate(CSHARP_PORT, pattern)
            sn = safe_name(pattern)
            out = RESULTS / "csharp" / f"{dataset}_{sn}.json"
            out.write_text(json.dumps(csharp_results[pattern], indent=2) if csharp_results[pattern] else "{}")

        cleanup()

        # ── Diff ──
        # Diff by index since Rust uses unescaped dots and C# uses escaped dots
        print(f"\n--- Diff: {dataset} ---")
        for i, (rust_pat, csharp_pat) in enumerate(zip(rust_patterns, csharp_patterns)):
            label = rust_pat
            d = diff_results(rust_results.get(rust_pat), csharp_results.get(csharp_pat), label)
            sn = safe_name(rust_pat)
            (RESULTS / "diff" / f"{dataset}_{sn}.json").write_text(json.dumps(d, indent=2))
            print(f"  {label}")
            if rust_pat != csharp_pat:
                print(f"    (C# pattern: {csharp_pat})")
            print(f"    Rust: {d['rust_count']}, C#: {d['csharp_count']}")
            print(f"    Rust-only: {d['rust_only_count']}, C#-only: {d['csharp_only_count']}")

    print(f"\n=== Done ===")
    print(f"Results in: {RESULTS}/")
    print(f"  rust/   — raw Rust provider output")
    print(f"  csharp/ — raw C# provider output")
    print(f"  diff/   — per-pattern diffs")


if __name__ == "__main__":
    main()
