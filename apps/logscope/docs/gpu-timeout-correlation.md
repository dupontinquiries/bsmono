# Asahi GPU timeouts during Scout — what the correlation shows

## The window

Diagnostic report stamps against `journalctl -k | rg 'GPU timeout'`, same session:

```
21:11:53   Scout mmap+retain completes          (diag @@STAGES)
21:13:20   first GPU timeout                    (journal)   ← 87 s later
21:16:51   last GPU timeout                     (journal)
21:17:20   profile line printed, generation ends (diag @@SUBMITS)
```

Timeouts begin **87 seconds after the model is mapped** and continue until roughly the end of
generation. They do not start at load and they do not start immediately.

## What precedes them

Reported from the run: **thousands of expert fills and 100+ GiB moved** before the first timeout. At
~68 MiB per fill that is on the order of 1500 fills.

This rules out two explanations and leaves one:

* **Not a single oversized dispatch.** Those fail on the first occurrence, not after 1500 successful
  ones. The prefill chunk and conversion chunking were already sized against the watchdog and behave
  the same at fill 1 as at fill 1500.
* **Not a cold-start driver problem.** The first 87 seconds of heavy GPU work complete cleanly.
* **Consistent with ACCUMULATION** — something that grows with fill count until the device degrades.
  Candidates, in the order worth testing:
  1. Retained bind groups and dims uniforms. `pending_keep` is only released on a poll, and with 99% of
     submits on the no-frame path the poll cadence is whatever `DIMS_RETAIN_CAP` forces.
  2. Buffer-object count. The governor's Metal/Vulkan budget models a soft cap; expert slot buffers plus
     per-dispatch dims could approach it over thousands of fills.
  3. Firmware queue depth on Honeykrisp specifically, where the submit governor disables itself because
     completion callbacks are slow or absent — so nothing throttles depth at all.

## What would distinguish them cheaply

Run the same generation with `LOGSCOPE_MOE_STREAM=0` on a model that fits resident. If the timeouts
disappear, the fill path is implicated rather than decode in general. If they persist, it is the
per-dispatch submit volume and not the fills.

The counter delta in `@@ERRORS` is the measure to compare across runs: the driver's `Timeout count:` is
cumulative, so only the delta says whether a change helped.
