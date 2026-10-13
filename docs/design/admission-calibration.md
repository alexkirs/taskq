# Admission calibration on the Mac host (D03)

Research for [#893](https://github.com/taskqorg/taskq/issues/893), plan batch 2 D03; input for WS5 host admission.
Target: [dynamic resources](dynamic-resources.md) R2. Authority: [taskq.md](../../taskq.md) § 3, § 4, § 9 until WS5 ships.
Budget rules: [specq performance](https://github.com/taskqorg/specq/blob/main/engineering/performance.md#budget).
No code or contract change here; numbers are one host, one day, under real owner load.

## Environment

- Host: Mac14,2 (M2 MacBook Air), 8 cores (4 performance + 4 efficiency), 24 GiB RAM, macOS 27.0.1.
- Disk: 1.8 TiB data volume, 325 GiB free (83% used).
- Load during the run: owner's desktop apps, Claude and Codex apps, six taskq services (one per project), 1–4 live taskq workers.
- Swap: 16.5–18.0 of 17–19 GiB used; `kern.memorystatus_vm_pressure_level` 2 (warning) at the end of the run.
- Release: taskq 4.2.0-dev, the `service run` processes of every project on this host.

## Method

1. Host sampler, 1028 s, 146 samples, every 7 s on average (the target was 1 s; `top -l 2` under load is slow):
   CPU busy = 100 − `top` idle %; memory available = (free + inactive + speculative pages) × page size / `hw.memsize`,
   the same read as the § 3 load gate; swap from `vm.swapusage`; live children of the taskq service.
2. Worker tree sampler, every 5–10 s over the same window: per service child, `phys_footprint` (`footprint -p`)
   summed over the child and all its descendants, plus summed `ps %cpu`. Covered 7 workers, 4 of them from their first 30 s.
3. Cold start: `claude -p --model haiku --tools ""` with a one-word prompt on stdin, three runs under `/usr/bin/time -l`.
4. Gate replay: one sample per 30 s through the § 3 rules (hold at CPU >= max or memory <= min;
   reopen after three samples at CPU <= max − 10 and memory >= min + 5), raw and with a 60 s mean CPU.
5. Service: `ps`, `footprint`, `lsof` of the running services; `/usr/bin/time -l` of `taskq status --json` and
   `taskq selftest` as one board pass; `launchctl limit maxfiles`; `du` of `<root>/.taskq`; code read of `src/service.rs`, `src/runtime.rs`.

## Measurements

### Host load

| Signal | p50 | p90 | p99 / max |
|---|---|---|---|
| CPU busy, raw sample | 65% | 93% | 100% |
| CPU busy, 30 s mean | 66% | 89% | 97% |
| CPU busy, 60 s mean | 70% | 90% | 94% |
| CPU busy, 300 s mean | 65% | 87% | 91% |
| Memory available (gate read) | 18.6% (4.5 GiB) | min 16.4% (3.9 GiB) | max 23.5% |

- Raw samples at or above 85%: 23%. Threshold flips over 1028 s: 30 s mean 8, 60 s mean 4, 300 s mean 1.
- Memory available reached 20% (the reopen level for `memory_min` 15) in 19% of samples, never three in a row at 30 s spacing.
- macOS reported 37% free (`memory_pressure`) while the gate read showed 17–18%: compressed memory and swap are not in the gate read.

### One worker (Claude runtime)

| Item | Value |
|---|---|
| Cold start to first answer | 8.9–9.7 s wall, 8.6–9.6 s CPU (about one core for 9 s) |
| Top process peak footprint at start | 146–160 MiB |
| Tree footprint 15–20 s after spawn | 410–480 MiB (Claude plus its MCP children) |
| Tree footprint, all samples | median 463 MiB, p90 792 MiB, max 1239 MiB (during a build) |
| Tree CPU in the first 30 s | 96–150% of one core, then 0–20% while waiting on the model |
| Codex CLI `--version` | 0.7 s, 13 MiB (Codex worker trees were not live during the run) |

### Gate replay (one sample per 30 s)

| `cpu_max` / `memory_min` | CPU input | Admission open |
|---|---|---|
| 85 / 15 (today's default) | raw | 0% |
| 85 / 15 | 60 s mean | 0% |
| 85 / 10 | 60 s mean | 51% |
| 90 / 12 | 60 s mean | 43% |
| 90 / 10 | 60 s mean | 57% |

Memory, not CPU, blocks the default: with `memory_min` 15 the reopen level 20% was not sustained on this host.

## Calibration (A1)

| Parameter | Measured | Recommendation |
|---|---|---|
| Baseline window | 60 s mean halves threshold flips of 30 s (8 → 4); 300 s reaches 1 but lags a 9 s launch spike by minutes | 60 s CPU mean; latest raw sample at 100% still vetoes |
| Sampling cadence | Launch transient lasts 15–30 s; 30 s point samples saw 13 of 35 above 85% | 30 s gate decision; feed the 60 s mean from 10 s reads |
| Launch spacing | Worker reaches its steady footprint 15–20 s after spawn; CPU spike ends within 30 s | One launch per 30 s host-wide, next only after a post-launch sample (keep § 3) |
| Memory floor | Steady worker 0.46 GiB, peak 1.24 GiB; gate read 3.9–5.6 GiB on this host | Hold below 10% and below 2.5 GiB available (two worker peaks); reopen at +5 points; also hold at `kern.memorystatus_vm_pressure_level` >= 4 (critical) |

## Service ceiling (A2)

Measured per project service (`taskq service run`), six running on this host.

| Resource | Measured | Ceiling | Behavior at the ceiling |
|---|---|---|---|
| CPU | 12–14 s CPU over 15 h (< 0.1% of one core); a board pass 0.2–1.5 s wall | 1% of one core, sustained | Pass rate is fixed by `poll` (60 s) and pokes; no extra work at load. Above it: lengthen `poll`, never skip heartbeats |
| Memory | 4.3 MiB footprint, 8–10 MiB RSS; one board pass peak 2–3 MiB footprint | 64 MiB footprint | Not enforced today. Target: log it and re-exec (§ 8 restart path); children survive through `.taskq/kids` |
| Descriptors | 6 open with 3 children (stdin, 2 logs, cwd, binary); child output goes to its log file, no pipe kept | launchd soft limit 256 | `spawn` fails with EMFILE: the service posts `gone attempt <id>` (`start failed: …`); 2nd `gone` moves the task to `ask` (§ 2). No incident report: environment error |
| Disk | `<root>/.taskq` 107 MiB, 227 attempt logs, largest 4.7 MiB, about 16 MiB/day over 6 days; events 30 days; requests newest 20 | 1 GiB per project, and 2 GiB free on the volume | Not enforced today: attempt logs, `cli-errors.log` and `trash/` grow unbounded. At ENOSPC `spawn` fails as above and `status.json` writes fail, so the tray shows stale. Target: hold new launches with reason `disk`, drop logs of closed tasks oldest first |

Per [specq performance](https://github.com/taskqorg/specq/blob/main/engineering/performance.md#budget) each bound
needs a named limit: attempt logs and `cli-errors.log` have none today. WS5 or a follow-up names them.

## Recommended Auto defaults for WS5 (A3)

| Default | Value | Risk |
|---|---|---|
| CPU close / reopen | 60 s mean >= 90% closes; <= 80% for three samples reopens | Owner's interactive work at 80–90% shares cores with new workers; a short burst under the 60 s mean is missed |
| Latest-sample veto | Raw sample at 100% holds that launch only | A single noisy sample delays one launch by 30 s |
| Memory close / reopen | Available < max(10%, 2.5 GiB) closes; >= 15% reopens | Gate read ignores compressed memory; on hosts with heavy swap the floor admits work while swap is nearly full |
| Memory pressure veto | `kern.memorystatus_vm_pressure_level` >= 4 holds | Level 2 (warning) is frequent here; using it would hold most of the day |
| Sampling | Read every 10 s, decide every 30 s, over 60 s old holds | Ten more reads per minute per service; six services read the same host, the shared WS5 authority should read once |
| Launch spacing | One launch per 30 s host-wide, post-launch sample required | Ten queued tasks need at least 5 min to start; a worker's later build spike (to 1.24 GiB) is not predicted |
| Startup | Held until three valid samples (90 s) | Service restart delays all launches by 90 s |
| Service budget | CPU 1%, memory 64 MiB, descriptors 256, disk 1 GiB per project | Not enforced in 4.2; disk is the only resource near a real limit today |

## Limits of this study

- One host, one load profile, 17 minutes; no idle-host run, no Windows. Windows qualification stays with WS5 (R-A8).
- Codex worker trees were not live; their footprint is unmeasured (Codex CLI start was 13 MiB).
- Sampler cadence was 7 s, not 1 s: short spikes inside 7 s are not in these numbers.
- Thresholds are percentages and GiB of this 24 GiB host; an 8 GiB host needs the GiB floor to dominate.
