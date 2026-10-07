# PM report schema v1

Immutable template v1. Source: accepted Wiki specification
05cf6aab1c6ed5fc9589b9e4673365cec34c58e6, Home section Versioned PM tick report contract (#191).
Changes require a new version/template; prompt version remains independent.

Every tick carries contract {version, sha256, source, template}, repository URL,
profile identity, Board URL or unavailable, observed_at UTC, outcome, actions,
refusals, source_status, workers, validation. Unknown/unavailable is explicit.
Each worker carries task link, title, state, runtime, machine, session link or
available attach/app reference, last_activity, event_at UTC or unknown,
commit link or unavailable. Observation freshness: 15 minutes; issue event age
is activity, not observation freshness. Future/invalid timestamps are blockers.

Show version/hash/source, repository/profile, observation/outcome, Board and
Workers table once, actions/refusals and source status. Empty workers: Workers:
none; failed source: Workers: unknown. Table-free channels use labeled lines
retaining every datum/link. PM adds judgment; it must not omit the generated
report on quiet passes. Data gaps block only dependent decisions.

Delivery is not receipt. Supported-channel readback JSON contains runtime,
session, source (channel message/session reference), received_at, applied_at,
report (tick report object), rendered (actual owner-channel output).
Verify with taskq report-verify FILE. Verification checks supplied evidence;
transport authenticity and live qualification are separate. Missing evidence
remains unknown/unqualified. No receipt database or inferred checkout ACK.

On mismatch: apply current payload at next safe tick, preserve work/ownership,
never repeat actions from a duplicate payload. Unsupported version blocks
obsolete report publication; run taskq update, retry safe tick. Interrupted
update retains execution; missing ACK is actionable unknown, never success.
