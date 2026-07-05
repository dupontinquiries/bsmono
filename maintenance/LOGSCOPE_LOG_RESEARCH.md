# Log systems: a categorization for the logscope Diagnose→development bridge

Research to ground the bridge from **Diagnose** (surgical log tool) into **active development**
(Chat palace / Forge). The bridge has to be *encompassing* — logs vary wildly by source, purpose,
and encoding, and a bridge that only understands one shape is useless. So first: how do logs
actually categorize? Three independent lenses (source, purpose, format) plus one cross-cutting
axis (severity). Then: what logscope actually meets, and what that means for the bridge.

## Lens 1 — by SOURCE (who emitted it)
The most common industry breakdown is six source-types (system, application, network, security,
transaction, audit), with observability treating **logs as one of three pillars** alongside
metrics and traces.

| Source type | Emitted by | Typical content |
|---|---|---|
| **System** | OS / kernel / init | boots, hardware faults, device errors, service start/stop |
| **Application** | app code | behavior, warnings, unhandled exceptions + stack traces, business events |
| **Network** | firewalls, switches, WAFs | connections, traffic, blocked ports, firewall actions |
| **Security** | auth stack, IDS/SIEM | failed logins, privilege escalation, intrusion attempts |
| **Transaction** | DBs, payment/order systems | queries, schema changes, financial operations |
| **Audit** | everything, deliberately | *who did what, when* — accountability, tamper-evident |

Key distinction for a dev tool: **application logs explain behavior; audit logs preserve
accountability.** They read differently and belong in different places — the bridge cares almost
entirely about the first (and system/build), because those are what you *act on in code*.

## Lens 2 — by PURPOSE (why it was collected)
Cuts across source. The "2 a.m. on-call" categories recur across incident reviews:
- **Debug/behavioral** — what the code did; the primary "what happened" signal. → **direct bridge
  target** (this is what becomes a fix).
- **Error/exception** — the highest-value dev signal: stack trace + request state + the inputs
  that produced it. → **the #1 bridge payload.**
- **Change/deploy** — version, timestamp, deploying user; ties an outage to a causal push. →
  bridge as *context* ("this broke after commit X").
- **Auth/access** — security's first line; usually *not* a dev action (→ security workflow).
- **Audit/accountability** — compliance/forensics; deliberate, protected, long-retained. → not a
  dev target; the bridge should *recognize and skip* these.
- **Performance** — resource/latency metrics derived from logs → capacity, not code fix.

## Lens 3 — by FORMAT (how it's encoded — this is what parsing keys on)
| Format | Shape | Where you meet it |
|---|---|---|
| **Plain text** | freeform line | legacy apps, `println`, `/var/log/*.log` |
| **syslog (BSD / RFC 5424)** | facility+severity+timestamp+host+msg (5424 adds structured-data) | the Unix logging lingua franca; rsyslog/syslog-ng |
| **journald** | structured, indexed **binary** (`journalctl`, `-o json`) | every systemd Linux system since ~2011 |
| **JSON** | one object per line; rich fields | modern apps, cloud, ELK/Splunk/Loki |
| **logfmt** | `key=value` pairs | Heroku-origin; Docker/k8s/microservices, Go |
| **CLF / Combined** | fixed positional web-access line | web servers (nginx/apache) |
| **CSV / XML / protobuf** | tabular / tagged / binary | exports, enterprise, high-throughput pipelines |

Two hard truths for a parser: **syslog defines transport, not message content** (endless in-body
variety), and **journald's on-disk format is binary + under-documented** (read via `journalctl`,
normalize with `-o json`). So a general tool parses the *envelope* it can, then treats the
message body as semi-structured.

## Cross-cutting axis — SEVERITY
OpenTelemetry's model is the useful common denominator: six tiers — TRACE, DEBUG, INFO, WARN,
ERROR, FATAL — over a 1–24 numeric range. ERROR = one operation failed but the service continues;
FATAL = the process itself can't recover. syslog has its own 0–7 facility/severity. **Severity is
the bridge's triage key**: ERROR/FATAL lines are the ones worth escalating into development;
INFO/DEBUG are context.

---

## What logscope actually meets (local Linux dev/sysadmin, not a SIEM)
logscope isn't a cloud observability backend — it's a *surgical local tool* on Asahi/postmarketOS.
So the relevant slice is narrower and concrete:
1. **journald / systemd** — the primary system source (`journalctl`, kernel + all services).
2. **`/var/log/*` text files** — messages, auth.log, Xorg, dpkg/pacman, dmesg.
3. **Application stdout/stderr** — the app you're developing, in whatever format it emits (often
   plain or JSON).
4. **Build/tool logs** — cargo/rustc, compiler output, test runners. *(logscope already treats
   these first-class: the Cargo tab + Forge.)*
5. **Container logs** — if present (docker/podman → often logfmt/JSON).

The purpose distribution here skews hard toward **error/exception + build failures + system
faults** — precisely the things that turn into code. Audit/security/transaction logs barely
appear on a dev box, so the bridge can safely optimize for the dev-actionable minority.

## Design implications for the Diagnose→development bridge
The research says the bridge needs three stages, and each maps to something logscope can build as
a pure, testable core (the pattern we've been using):

1. **Detect the envelope (format).** A `LogFormat` classifier: sniff JSON (`{`), logfmt (`k=v`),
   syslog (RFC3164/5424 prefix), journald (via `journalctl -o json`), else plain. Cheap, pure,
   testable. Makes the tool *encompassing* by construction — it degrades to plain-text/regex when
   it can't recognize the shape (which is the honest floor, matching how `shorten`/`probe`
   already work).
2. **Extract the signal (purpose + severity).** From a line/region, pull: severity (map syslog
   0–7 and text ERROR/WARN to the OTel tiers), a message, and — the money field — any **stack
   trace / error identifier / failing symbol**. This is the payload the palace/Forge act on.
3. **Frame for the target.** The bridge hands the extracted signal to the right tool with the
   right framing:
   - → **Chat (palace):** "here's the error + surrounding context — help me plan a fix" (open-
     ended, exploratory; stage the line **plus** its context window, conversational).
   - → **Forge:** "here's the failing symbol/region — operate on the block" (surgical; jump to
     the AST bloc, one-shot action).
   - The *severity + purpose* decide the default: ERROR/exception → offer both; build failure →
     Forge/Cargo; system fault → Chat with system context.

**Encompassing without being open-ended:** the user's instinct was right — not "parse anything"
but "recognize the common envelopes, and fall back to plain text gracefully." That's a **bounded,
extensible** classifier (add formats as encountered) rather than an infinite parser. The category
model above is the contract; the format sniffer + signal extractor are the two pure cores to
build next (both lean-buildable + testable, no GUI).

## Open questions this raises (for you)
- Which of the five local sources do you hit most — journald, app stderr, or build logs? (Order
  of the sniffer/extractor work.)
- For the bridge default: should ERROR lines *auto-offer* both Chat and Forge, or stay a manual
  "send to…" action? (Ties back to the earlier log→code bridge question.)
- Should the bridge ever touch audit/security logs, or explicitly stay out of them (dev focus)?

## Sources
NIST SP 800-92 (security log management); OWASP Logging Cheat Sheet; OpenTelemetry logs/severity
model; Coralogix "Application Logging Best Practices 2026"; Graylog / Last9 / SigNoz log-format
guides; ManageEngine & nickb.dev on syslog vs journald; edgedelta / webasha log-type guides;
Google Cloud & Palantir audit-log docs; Netdata structured-logging formats.
