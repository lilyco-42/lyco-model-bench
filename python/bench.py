#!/usr/bin/env python3
"""lyco-model-bench -- daily model discovery + token-speed benchmark.

For OpenAI-compatible providers (NIM / OpenRouter) this tool:

  1. discovers the *current* model list from the provider's /models endpoint,
  2. cheaply PROBES every candidate (1 output token, short timeout) so models
     that are listed-but-disabled or that hang are filtered out fast,
  3. BENCHMARKS the responsive ones with a fixed Chinese prompt, repeated
     --reps times, reporting median TTFT / total latency / output tokens/sec,
  4. writes a timestamped JSON report plus results/latest.md and latest.json.

Providers constantly add/remove/rename models, so this is meant to run daily.

Keys are read from a keys.env file (KEY=VALUE) or the environment and are never
printed. Stdlib only, so the same file runs on Windows and on the Radxa board.

Usage:
  python bench.py                                   # discover + probe + bench, both providers
  python bench.py --provider nim                    # just NIM
  python bench.py --models a,b --reps 3             # bench explicit models, skip discovery
  python bench.py --dry-run                         # list candidates only
"""
import argparse
import concurrent.futures as cf
import json
import os
import pathlib
import re
import ssl
import statistics
import time
import urllib.error
import urllib.request

DEFAULT_KEYS = pathlib.Path.home() / ".config" / "lyco-llm" / "keys.env"

PROVIDERS = {
    "nim": {"base": "https://integrate.api.nvidia.com/v1", "env": "NIM_API_KEY"},
    "openrouter": {"base": "https://openrouter.ai/api/v1", "env": "OPENROUTER_API_KEY"},
}

PROMPT = "用简体中文写一段描写春天西湖的短文，大约120字。"
SYS = "你是简洁的中文助手，只用简体中文回答。"

# Models known to be good QA backends; always (re)tested even if discovery misses them.
QA_HINTS = {
    "nim": [
        "nvidia/nemotron-3-super-120b-a12b",
        "meta/muse-glimmer-30b",
        "openai/gpt-oss-20b",
        "meta/llama-3.2-11b-vision-instruct",
        "nvidia/ising-calibration-1.5-31b",
    ],
    "openrouter": [
        "nvidia/nemotron-3-super-120b-a12b:free",
        "openai/gpt-oss-20b:free",
        "google/gemma-4-26b-a4b-it:free",
    ],
}

CJK = re.compile(r"[\u4e00-\u9fff]")


def load_keys():
    path = pathlib.Path(os.environ.get("LYCO_LLM_KEYS", str(DEFAULT_KEYS)))
    keys = {}
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            keys[k.strip()] = v.strip().strip('"').strip("'")
    for k in ("NIM_API_KEY", "OPENROUTER_API_KEY"):
        if os.environ.get(k):
            keys[k] = os.environ[k]
    return keys


def _open(url, key, method="GET", payload=None, timeout=90, stream=False):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", "Bearer " + key)
    req.add_header("Content-Type", "application/json")
    if stream:
        req.add_header("Accept", "text/event-stream")
    if "openrouter" in url:
        req.add_header("HTTP-Referer", "https://github.com/lyco-42")
        req.add_header("X-Title", "lyco-model-bench")
    return urllib.request.urlopen(req, timeout=timeout, context=ssl.create_default_context())


def list_models(provider, key, timeout=30):
    base = PROVIDERS[provider]["base"]
    with _open(base + "/models", key, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8", "replace")).get("data", [])


def discover(provider, key, limit, only_free):
    """Return (candidate_ids, total_listed)."""
    models = list_models(provider, key)
    if provider == "openrouter":
        picked = set()
        for m in models:
            mid = m.get("id", "")
            price = (m.get("pricing") or {}).get("prompt", "1")
            try:
                free = float(price) == 0.0
            except (TypeError, ValueError):
                free = False
            if mid.endswith(":free") or (only_free and free):
                picked.add(mid)
        picked.update(QA_HINTS[provider])
        ids = sorted(picked)
        return (ids[:limit] if limit > 0 else ids), len(models)
    ids = sorted({m.get("id", "") for m in models if m.get("id")})
    for h in QA_HINTS[provider]:
        if h not in ids:
            ids.append(h)
    return (ids[:limit] if limit > 0 else ids), len(models)


def _post(base, key, payload, timeout, stream):
    return _open(base + "/chat/completions", key, "POST", payload, timeout, stream)


def probe(provider, key, model, timeout):
    """Tiny non-streaming call: is the model enabled / responsive right now?"""
    base = PROVIDERS[provider]["base"]
    payload = {"model": model, "messages": [{"role": "user", "content": "hi"}],
               "max_tokens": 1, "stream": False}
    t0 = time.time()
    try:
        with _post(base, key, payload, timeout, False) as r:
            r.read()
        return {"model": model, "ok": True, "ms": int((time.time() - t0) * 1000)}
    except urllib.error.HTTPError as e:
        return {"model": model, "ok": False, "ms": int((time.time() - t0) * 1000),
                "err": "HTTP %s %s" % (e.code, e.read().decode("utf-8", "replace")[:80])}
    except Exception as e:  # noqa: BLE001
        return {"model": model, "ok": False, "ms": int((time.time() - t0) * 1000), "err": str(e)[:90]}


def _one_streaming_run(provider, key, model, max_tokens, timeout):
    base = PROVIDERS[provider]["base"]
    payload = {"model": model, "messages": [{"role": "system", "content": SYS},
                                            {"role": "user", "content": PROMPT}],
               "max_tokens": max_tokens, "temperature": 0.2, "stream": True}
    t0 = time.time()
    ttft_content = None
    ttft_reason = None
    content = ""
    reason = ""
    a_chunks = 0
    r_chunks = 0
    usage = None
    with _post(base, key, payload, timeout, True) as r:
        for raw in r:
            line = raw.decode("utf-8", "replace").strip()
            if not line.startswith("data:"):
                continue
            body = line[5:].strip()
            if body == "[DONE]":
                break
            try:
                j = json.loads(body)
            except ValueError:
                continue
            if j.get("usage"):
                usage = j["usage"]
            choices = j.get("choices") or []
            if not choices:
                continue
            ch = choices[0]
            delta = ch.get("delta") or {}
            msg = ch.get("message") or {}
            rc = delta.get("reasoning_content") or msg.get("reasoning_content")
            if rc:
                if ttft_reason is None:
                    ttft_reason = time.time() - t0
                reason += rc
                r_chunks += 1
            c = delta.get("content")
            if not c:
                # Some servers ignore stream=true and return a normal body.
                c = msg.get("content") or ch.get("text")
            if c:
                if ttft_content is None:
                    ttft_content = time.time() - t0
                content += c
                a_chunks += 1
    total = time.time() - t0
    tokens = (usage or {}).get("completion_tokens") or (a_chunks + r_chunks)
    return {"total": total, "ttft_content": ttft_content, "ttft_reason": ttft_reason,
            "tokens": tokens, "content": content, "reason": reason,
            "answer_chunks": a_chunks, "reason_chunks": r_chunks,
            "streamed": ttft_content is not None or ttft_reason is not None}


def bench(provider, key, model, max_tokens, timeout, reps):
    base = PROVIDERS[provider]["base"]
    runs = []
    t0 = time.time()
    for _ in range(reps):
        try:
            runs.append(_one_streaming_run(provider, key, model, max_tokens, timeout))
        except urllib.error.HTTPError as e:
            return {"model": model, "ok": False, "err": "HTTP %s %s" % (e.code, e.read().decode("utf-8", "replace")[:100]),
                    "total_ms": int((time.time() - t0) * 1000)}
        except Exception as e:  # noqa: BLE001
            if not runs:
                return {"model": model, "ok": False, "err": str(e)[:120],
                        "total_ms": int((time.time() - t0) * 1000)}
    if not runs:
        return {"model": model, "ok": False, "err": "no runs", "total_ms": int((time.time() - t0) * 1000)}
    totals = [r["total"] for r in runs]
    tokens = statistics.median([r["tokens"] for r in runs])
    best = max(runs, key=lambda r: len(r["content"]))  # richest answer sample
    tps = [r["tokens"] / r["total"] for r in runs if r["total"] > 0 and r["tokens"]]
    ans_ttfts = [r["ttft_content"] for r in runs if r["ttft_content"] is not None]
    reason_ttfts = [r["ttft_reason"] for r in runs if r["ttft_reason"] is not None]
    return {
        "model": model, "ok": True,
        "reps": len(runs),
        "total_ms": int(statistics.median(totals) * 1000),
        "tok_s": round(statistics.median(tps), 1) if tps else 0.0,
        "ttft_ms": int(statistics.median(ans_ttfts) * 1000) if ans_ttfts else None,
        "reason_ttft_ms": int(statistics.median(reason_ttfts) * 1000) if reason_ttfts else None,
        "out_tokens": tokens,
        "answer_tokens": max(r["answer_chunks"] for r in runs),
        "reason_chars": len(best["reason"]),
        "streamed": best["streamed"],
        "chinese": bool(CJK.search(best["content"])),
        "answered": bool(best["content"].strip()),
        "chars": len(best["content"]),
        "preview": best["content"][:60].replace("\n", " "),
    }


def quality(provider, key, model, max_tokens, timeout):
    """One non-streaming Chinese answer, for eyeballing quality."""
    base = PROVIDERS[provider]["base"]
    q = "请用简体中文介绍一下杭州西湖，并说明它为什么出名。"
    payload = {"model": model, "messages": [{"role": "system", "content": SYS},
                                            {"role": "user", "content": q}],
               "max_tokens": max_tokens, "temperature": 0.3, "stream": False}
    t0 = time.time()
    try:
        with _post(base, key, payload, timeout, False) as r:
            d = json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        return {"model": model, "err": "HTTP %s" % e.code, "ms": int((time.time() - t0) * 1000)}
    except Exception as e:  # noqa: BLE001
        return {"model": model, "err": str(e)[:100], "ms": int((time.time() - t0) * 1000)}
    txt = (((d.get("choices") or [{}])[0].get("message") or {}).get("content")) or ""
    return {"model": model, "ms": int((time.time() - t0) * 1000), "text": txt}


def _pool(items, worker, concurrency, on_done):
    results = []
    with cf.ThreadPoolExecutor(max_workers=concurrency) as ex:
        futs = [ex.submit(worker, i) for i in items]
        for fut in cf.as_completed(futs):
            row = fut.result()
            results.append(row)
            on_done(row)
    return results


def recommend(rows):
    """Fastest model that actually answered, in Chinese."""
    good = [r for r in rows if r.get("ok") and r.get("answered") and r.get("chinese")]
    return max(good, key=lambda r: r.get("tok_s") or 0) if good else None


def render_md(rows, when):
    ranked = sorted(rows, key=lambda r: (not r["ok"], -(r.get("tok_s") or 0)))
    out = ["# Model speed report -- %s" % when, "",
           "| provider | model | ok | answered | chinese | tok/s | ttft(ms) | total(ms) | tokens | answer | reason_chars |",
           "|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in ranked:
        out.append("| %s | `%s` | %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (
            r["provider"], r["model"], "yes" if r["ok"] else "no",
            "yes" if r.get("answered") else "no", "yes" if r.get("chinese") else "no",
            r.get("tok_s", ""), r.get("ttft_ms") if r.get("ttft_ms") is not None else "",
            r.get("total_ms", ""), r.get("out_tokens", ""),
            r.get("answer_tokens", ""), r.get("reason_chars", "")))
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--provider", choices=["nim", "openrouter", "both"], default="both")
    ap.add_argument("--limit", type=int, default=0, help="max candidates per provider (0 = all)")
    ap.add_argument("--models", default="", help="explicit comma-separated ids (single provider, skips discovery)")
    ap.add_argument("--max-tokens", type=int, default=512,
                    help="output budget; must be large enough for reasoning models to finish and answer")
    ap.add_argument("--timeout", type=float, default=60, help="benchmark request timeout")
    ap.add_argument("--probe-timeout", type=float, default=10)
    ap.add_argument("--reps", type=int, default=2)
    ap.add_argument("--concurrency", type=int, default=8)
    ap.add_argument("--no-free-only", dest="only_free", action="store_false", default=True)
    ap.add_argument("--no-probe", action="store_true", help="benchmark all candidates without probing")
    ap.add_argument("--quality", action="store_true",
                    help="print full Chinese answers for the live models instead of speed numbers")
    ap.add_argument("--out", default="")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    keys = load_keys()
    providers = ["nim", "openrouter"] if args.provider == "both" else [args.provider]
    when = time.strftime("%Y-%m-%d %H:%M:%S")
    all_rows = []
    probed_out = []

    for provider in providers:
        key = keys.get(PROVIDERS[provider]["env"], "")
        if not key:
            print("!! no key for %s (%s) -- skipped" % (provider, PROVIDERS[provider]["env"]))
            continue
        if args.models:
            candidates, total = [m.strip() for m in args.models.split(",") if m.strip()], 0
        else:
            candidates, total = discover(provider, key, args.limit, args.only_free)
        print("== %s: %d/%d candidates" % (provider, len(candidates), total))
        if args.dry_run:
            for m in candidates:
                print("   ", m)
            continue

        if args.no_probe:
            live = candidates
        else:
            print("-- probing %d models (timeout %ss)..." % (len(candidates), args.probe_timeout))
            pr = _pool(candidates, lambda m: probe(provider, key, m, args.probe_timeout),
                       args.concurrency, lambda r: None)
            live = [r["model"] for r in pr if r["ok"]]
            dead = [r for r in pr if not r["ok"]]
            probed_out.append({"provider": provider, "live": len(live), "dead": len(dead),
                               "dead_models": sorted(r["model"] for r in dead)})
            print("   live=%d dead=%d" % (len(live), len(dead)))

        if args.quality:
            print("-- quality answers from %d live models..." % len(live))
            qr = _pool(live, lambda m: quality(provider, key, m, max(256, args.max_tokens), args.timeout),
                       args.concurrency, lambda r: None)
            for r in sorted(qr, key=lambda x: x.get("err") is not None):
                if r.get("err"):
                    print("\n### %s [%s]\n  ERROR %s" % (r["model"], r["ms"], r["err"]))
                else:
                    print("\n### %s [%s ms]\n%s" % (r["model"], r["ms"], r["text"][:400].replace("\n", " ")))
            continue

        print("-- benchmarking %d models (reps=%d)..." % (len(live), args.reps))
        rows = _pool(live, lambda m: bench(provider, key, m, args.max_tokens, args.timeout, args.reps),
                     args.concurrency,
                     lambda r: print("   [%s] %-50s %s %s tok/s" % (
                         provider, r["model"], "ok " if r["ok"] else "ERR",
                         r.get("tok_s", "")), flush=True))
        for r in rows:
            r["provider"] = provider
        all_rows += rows

    if args.dry_run or not all_rows:
        return

    results = pathlib.Path(args.out or (pathlib.Path(__file__).parent / "results"))
    results.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y-%m-%d_%H%M")
    payload = {"when": when, "rows": all_rows, "discovery": probed_out}
    text = json.dumps(payload, ensure_ascii=False, indent=2)
    (results / ("%s.json" % stamp)).write_text(text, encoding="utf-8")
    (results / "latest.json").write_text(text, encoding="utf-8")
    md = render_md(all_rows, when)
    (results / "latest.md").write_text(md, encoding="utf-8")
    rec = recommend(all_rows)
    if rec:
        (results / "recommend.json").write_text(json.dumps(rec, ensure_ascii=False, indent=2), encoding="utf-8")
        print("\nRECOMMEND: %s / %s  (%s tok/s, total %sms, answers=yes, chinese=yes)" % (
            rec["provider"], rec["model"], rec.get("tok_s"), rec.get("total_ms")))
    else:
        print("\nRECOMMEND: (none -- no model answered in Chinese)")
    print("\n" + md)


if __name__ == "__main__":
    main()
