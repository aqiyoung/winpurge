# WinPurge

Safely see what is eating your Windows system drive, and clean it up without
breaking Windows.

Windows 10/11 systems tend to accumulate tens of gigabytes of temporary files,
update caches, browser caches and crash dumps. Disk Cleanup misses a lot of it,
and manual deletion is risky. WinPurge shows you where the space went and only
removes things that are safe to remove — and even then it *moves* them to a
quarantine folder instead of deleting them, so you can put everything back.

## What it does

- **Space usage analysis** — scans a drive and shows a size tree: which
  directories hold how much, with proportional bars.
- **Junk detection** — measures a fixed whitelist of known-junk locations.
- **Safe cleaning** — moves selected items to
  `%ProgramData%\WinPurge\Quarantine\<timestamp>\`, never deletes.
- **One-click restore** — every batch has a manifest; put anything back.
- **Dry-run by design** — cleaning starts from an explicit confirmation dialog
  that lists exactly what will be moved.

## Safety model

This tool touches files on your system, so the design is deliberately
conservative:

1. **Whitelist only.** `src/rules.rs` is the single source of truth for what may
   ever be touched. There is no "delete arbitrary path" code path anywhere.
2. **Second line of defence.** `rules::is_critical` refuses volume roots, the
   top-level `Windows`, `Program Files` and `Users` trees, and core files such
   as `pagefile.sys` / `hiberfil.sys`.
3. **No hard deletes.** Everything goes to quarantine first.
4. **Reparse points are never followed.** Junctions, symlinks and OneDrive
   placeholders are skipped, so space is never counted twice and scans cannot
   loop.
5. **Elevation is explicit.** System-wide categories are disabled unless the
   process runs elevated, and the UI says so.
6. **Nothing pre-selected.** Cleaning requires ticking categories and confirming
   a list of paths.

## Categories

| Category | Location | Admin |
|---|---|---|
| User temporary files | `%TEMP%` | no |
| Windows temporary files | `%SystemRoot%\Temp` | yes |
| Windows Update download cache | `%SystemRoot%\SoftwareDistribution\Download` | yes |
| Windows Error Reporting | `%ProgramData%\Microsoft\Windows\WER` | yes |
| User crash dumps | `%LOCALAPPDATA%\CrashDumps` | no |
| Thumbnail / icon cache | `%LOCALAPPDATA%\Microsoft\Windows\Explorer` | no |
| Edge / Chrome cache | per-profile `Cache`, `Code Cache`, `GPUCache` | no |
| Firefox cache | per-profile `cache2` | no |
| Recycle Bin | `$Recycle.Bin` on every volume | no |
| Windows.old | `%SystemDrive%\Windows.old` | yes |

## Install

Grab `winpurge.msi` from the [Actions artifacts](https://github.com/aqiyoung/winpurge/actions)
or from a release. Installing is per-machine and requires administrator rights;
run WinPurge itself elevated if you want to clean the system-wide categories.

## Build

Builds happen in GitHub Actions on `windows-latest` (`cargo fmt --check`,
`cargo build --release`, `cargo test --release`, then WiX for the `.msi`).
There is intentionally no local build step.

The installer is built with WiX **v6**, pinned in `.github/workflows/ci.yml`.
WiX v7 introduced an Open Source Maintenance Fee EULA, so the workflow stays on
the open-source (LGPL) release.

Artifacts of every run (`winpurge.exe` and `winpurge.msi`) are attached to the
run under **Actions → CI → Artifacts**.

## Development

```bash
cargo fmt          # format
cargo test         # engine + rule unit tests
```

The unit tests cover the parts that matter for safety: path predicates,
critical-path refusal, quarantine round trips, and per-test fixture isolation.

## Disclaimer

This tool modifies your file system. It is provided as-is, without warranty.
Restoring from quarantine requires the quarantine folder to be intact; deleting
it loses the ability to roll back.
