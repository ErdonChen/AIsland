# Changelog

All notable changes to AIsland Community Edition will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Open-source governance, privacy, security, contribution, support, and trademark documentation.
- Reproducible Windows source-build instructions and automated quality gates.

### Changed

- Public repository and updater links now use `ErdonChen/AIsland`.

## [0.1.0-preview.6] - 2026-08-20

### Added

- Added six application background color presets under Display and Appearance,
  each with a circular preview swatch and independent glass transparency.
- Added manual microphone recording to Daily Notes. Recordings are stored
  locally by note date, support multiple clips, and can be played or deleted
  independently without requiring a text note.
- Added a collapsible, Monday-first six-week calendar to Daily Notes, including
  month navigation, keyboard navigation, and markers for dates containing text
  or recordings.

### Changed

- Connected the Elastic, Smooth, and Swift expansion choices to distinct native
  window animations and added an in-settings preview action.
- Daily Notes now finish pending text saves and active recordings before date
  or page navigation; recoverable failures keep the user on the current note.

### Fixed

- Prevented encoder failures and incomplete recording assets from appearing as
  successful playable recordings.

## [0.1.0-preview.5] - 2026-08-19

### Added

- Replaced the four fixed window-scale presets with a continuous 80%-220%
  slider and live preview, providing a practical range for 2K and 4K displays.
- Added horizontal and corner resizing for the floating capsule, with separate
  width memory for collapsed and expanded layouts.
- Added a top-edge tuck control that reduces the capsule to a visible black
  strip and restores it when the pointer returns.

### Changed

- Made collapsed capsule information responsive across narrow, medium, and wide
  widths. Agent icons, status indicators, status text, and running or attention
  state remain visible, while extra agents collapse into `+N` as space runs out.
- Increased the hover expansion delay to 600 ms and cancelled pending expansion
  when resizing or using the tuck control, leaving enough time to operate the
  capsule without accidental expansion.

## [0.1.0-preview.4] - 2026-08-18

### Fixed

- Fixed a transparent-window bug where focusing the desktop or changing display scaling, then clicking the island, could restore the native Windows title bar and borders. The main window now intercepts `WM_NCCALCSIZE` / `WM_NCACTIVATE` / `WM_NCPAINT` through a window-procedure subclass and reasserts the borderless style on focus, resize, and DPI events. See `docs/fix-transparent-window-chrome.md` for the full investigation.

## [0.1.0-preview.3] - 2026-08-17

### Fixed

- Fixed a transparent-window bug where changing glass transparency could restore the native Windows title bar and window controls.

## [0.1.0] - Unreleased

The first Community Edition source release is being prepared. No installer or portable executable will be published until the Windows artifacts have a trusted Authenticode signature and pass the release verification workflow.
