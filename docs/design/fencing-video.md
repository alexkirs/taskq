# Historical video writers: #801

The historical-writer hold is **satisfied by owner policy**, not by technical
revocation. [The owner lifted it](https://github.com/alexkirs/taskq/issues/801#issuecomment-6134964702)
on 2026-10-13: #584–#591 are dead work; restart through #802, #803 and #805.
Every currently running worker is the owner's. Tool/provider launch problems
become numbered asks in those tasks, not a renewed historical-writer hold.

## Independently checked evidence

Read #795, #801, #811 and #812 bodies and logs, the retained #584–#591
identity records and spawn logs, and both merged host reports on 2026-10-13.
The reports, rather than task closure, supply the observations:

- [Mac report](fencing-mac.md), [PR #813](https://github.com/alexkirs/taskq/pull/813),
  merged as `66df70f6c729c8ea13ebccda974570399bf9e2e3`.
  [Owner policy acceptance](https://github.com/alexkirs/taskq/issues/811#issuecomment-6134962807).
- [Windows report](fencing-win.md), [PR #816](https://github.com/alexkirs/taskq/pull/816),
  merged as `d79b763bb61c753b6df8ae0fd8cb7ba2527162fc`.
  [Owner attestation](https://github.com/alexkirs/taskq/issues/812#issuecomment-6134963790).

This review checks retained evidence; it does not repeat process inspection or
provider checks. No session, credential, old task or asset was changed.

## Historical writer inventory

Identifiers below are public session prefixes from issue records, not credentials.
Each row is **fenced by owner policy**. Individual session termination or inability
to write remains **unknown technically**: neither host report proves revocation
for every recorded identity. A `gone` entry or closed task is not that proof.

| Writer | Recorded identity prefixes | Evidence / limitation |
|---|---|---|
| Video PM, mac | `01a12181-4f3d`; coordinating spawn author `01a11dc5` | #584–#591 logs; mac report finds no historical taskq writer |
| #584 workers / supervisor | `01a1218a-7019`, `01a121a3-14ad` / `01a12189-eceb` | #584 identity record and spawn log |
| #585 worker / supervisor | `01a121a6-4a2c` / `01a121a5-8d93` | #585 identity record and spawn log |
| #586 workers / supervisor | `01a121a6-69a2`, `01a121af-2116` / `01a121a5-ae56` | #586 identity record and spawn log |
| #587 worker / supervisor | `01a121a6-daa7` / `01a121a5-ca93` | #587 spawn log |
| #588 worker / supervisor | `01a121ac-a8ee` / `01a121ab-f05a` | #588 spawn log |
| #589 worker / supervisor | `01a12260-d6bc` / `01a12260-2185` | #589 identity record and spawn log |
| #590 workers / supervisor | `01a12264-d0a4`, `01a1227c-8593` / `01a12264-2ecb` | #590 spawn log |
| #591 worker / supervisor | `01a12265-3a1f` / `01a12264-910d` | #591 spawn log |
| WIN_CDX3 / historical Windows coordinators | WIN_CDX3; exact session mapping unknown | #587/#590/#591 hold records; #812 owner attests no historical video writer alive |
| Other historical Codex/Claude PM, supervisor or worker sessions | Complete identity mapping unknown | Mac report retains a live historical Codex daemon and 10-11 thread; Windows session ages unobserved. Owner accepts both hosts by policy |

## Git/provider rights

Every row is accepted as **fenced by owner policy** for this hold. The technical
column preserves the observed live rights and unknowns; policy does not mean
credentials were removed or providers are ready for production.

| Right | Mac evidence | Windows evidence | Technical fencing |
|---|---|---|---|
| GitHub / Git push | Owner login, admin/push on taskq; HTTPS helpers present | Login with repo/workflow scopes; HTTPS/GCM present; push not exercised in host inspection | Unknown; credentials remain live |
| SSH / deploy keys | No agent identities, private-key files present; taskq deploy keys absent | Private-key files present; deploy-key authorization unknown | Mac taskq deploy keys fenced by absence; other key grants unknown |
| OpenAI / ImageGen | ChatGPT login present | Environment API key absent; app/browser login unknown | Unknown; mac login remains live |
| Google / NotebookLM | Drive login items present; NotebookLM browser login unknown | Environment keys absent; app/browser login unknown | Unknown |
| Meshy | Keychain API key reachable from running owner app | Environment key absent; app/browser login unknown | Unknown; mac key remains live |

No missing technical evidence is treated as PASS. The owner explicitly accepted
these unknowns and live owner rights; no additional audit or fencing action is
required for #801. Creative approval, cost authorization and final acceptance
remain with the continuation tasks. Historical candidates are not accepted finals.
