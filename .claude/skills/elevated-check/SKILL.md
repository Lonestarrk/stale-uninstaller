---
name: elevated-check
description: Use when a live check on the development machine needs administrator rights — running a dump mode elevated, screenshotting Stale Uninstaller, reading the real quarantine or other admin-only state
---

# Elevated checks on the development machine

Stale needs administrator rights, and the agent's shells are not elevated (`cargo run -p stale-gui`
fails with os error 740). Elevated work goes through a runner the user set up once: a scheduled task
`StaleDevElevated` with highest privileges that runs `.superpowers/elevated/run.ps1`. If the task
does not exist (`schtasks /query /tn StaleDevElevated` fails), ask the user to create it; never try
to get around UAC.

## What may run there

Only read-only commands (dump modes, listings, screenshots), the end-to-end tests the user agreed to,
and the installer smoke test from the release skill. Nothing that removes, quarantines or changes the
user's data. `%ProgramData%\Stale Uninstaller` is real data: its quarantine belongs to the user, and
just starting Stale runs the automatic purge of quarantines older than the configured age.

## Running a job

1. Write the job to `.superpowers/elevated/jobs/NNN-<what>.ps1`, NNN one higher than the newest file
   in `.superpowers/elevated/done/`. Start it with `. 'H:\Projects\uninstaller\.superpowers\elevated\lib.ps1'`
   when it needs the helpers. Print everything you want to see; output goes to the log.
2. `powershell -NoProfile -Command "schtasks /run /tn StaleDevElevated"`.
3. Poll `.superpowers/elevated/log.txt` until the section that starts `--- job NNN-<what>.ps1` has an
   `=== end` line (a few seconds; a cold `dump space` takes about a minute), then read that section.
   Finished jobs move to `done/` with a timestamp, so earlier jobs are templates for new ones.

`lib.ps1` helpers: `Start-Stale` (starts `target\release\StaleUninstaller.exe`, returns `@{ Proc; Hwnd }`),
`Save-Shot $hwnd 'name'` (PrintWindow capture into `.superpowers/elevated/shots/name.png`;
open it with Read and look before concluding anything), `Stop-Stale $s` (takes that hashtable), `[G]::MainWindow(pid)`.

## Limits

- Posted mouse clicks and keys do not reach egui, so a job can launch and screenshot Stale but not
  click through it. Views behind a click (ghosts, Sök rester, popups) need the user to test them, or
  logic tests in `stale-gui`; say plainly which parts were not checked live.
- Output from the real machine contains the user's folder and account names. Anonymise them (for
  example `H:\<user folder 1>`, `C:\Users\<account 1>\`) before anything goes into `docs/research/`
  or another committed file, and never send them to an external service. Python one-liners mangle
  Windows backslashes: write a small script with `chr(92)` or raw strings instead.
