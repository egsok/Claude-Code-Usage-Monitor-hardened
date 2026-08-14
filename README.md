![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Claude Code Usage Monitor Hardened

[English](#english) · [Русский](#русский)

This is a security-focused fork of
[CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor).
The original project provides a useful native Windows taskbar monitor. This fork
keeps that functionality while narrowing what a passive monitoring utility is
allowed to execute and update on the user's machine.

![Screenshot](.github/animation.gif)

## English

A lightweight Windows taskbar widget for people already using Claude Code, with optional Codex and Google Antigravity usage display.

It sits in your taskbar and shows how much of your Claude Code, Codex, and/or Antigravity usage window you have left, without needing to open the terminal or the provider site.

### Why This Fork Exists

During a source review, we found two behaviors that were useful for convenience
but broader than we want from a passive usage monitor:

1. When credentials expired, the original app could start Claude Code or Codex
   CLI commands in the inherited working directory to trigger an authentication
   refresh. Agent CLIs can load project-local configuration, instructions, and
   hooks, so launching them implicitly creates behavior beyond simply reading
   usage information.
2. The portable self-updater could download the executable from the latest
   GitHub release and replace the running binary without a separately verified
   digital signature or checksum.

These behaviors appeared to be convenience features; we found no indication of
malicious intent. This fork simply adopts a narrower trust model.

### Hardened Changes

- Never launches Claude Code or Codex CLI commands. If authentication expires,
  the app reports the problem and waits for the user to log in manually.
- Disables executable download and replacement for portable builds.
- Keeps release checks informational. A dedicated WinGet package will be the
  only supported automatic update path once it is published.
- Includes a regression test that guards against reintroducing background agent
  CLI launches.
- Uses a repository and future package identity separate from the upstream app.

This remains a local credential-reading utility: it must read provider OAuth
credentials and send them to the corresponding official usage endpoints. See
[Privacy And Security](#privacy-and-security) for the exact data flow.

## What You Get

- A **5h** bar for your current 5-hour Claude usage window
- A **7d** bar for your current 7-day window
- Optional Codex usage bars alongside Claude Code
- Optional Antigravity model usage bars for Google's 5-hour and weekly Gemini quota windows
- A live countdown until each limit resets
- A small native widget that lives directly in the Windows taskbar
- System tray icon badges showing your enabled model usage percentage
- Left-click the tray icon to toggle the taskbar widget on or off
- Right-click options for refresh, displayed models, update frequency, language, startup, widget visibility, and updates
- Multi-monitor taskbar placement, so the widget can live on the taskbar for the screen you prefer

## Who This Is For

This app is for Windows users who already have **Claude Code (CLI or App) installed and signed in**.

Codex support is optional. To show Codex usage, install and sign in to the Codex CLI, then enable Codex from the right-click **Models** menu.

Antigravity support is optional too. To show Antigravity usage, install and sign in to Google Antigravity, then enable the **Antigravity** model from the right-click **Models** menu.

It works best if you want a simple "how close am I to the limit?" display that is always visible.

## Requirements

- Windows 10 or Windows 11
- Claude Code (CLI or App) installed and authenticated
- Optional: Codex CLI installed and authenticated, if you want Codex usage
- Optional: Google Antigravity installed and authenticated, if you want Antigravity usage

If you use Claude Code through WSL, that is supported too. The monitor can read your Claude Code credentials from Windows or from your WSL environment.

## Install

The dedicated hardened WinGet package is not published yet. Until it is available,
build from source or download `claude-code-usage-monitor.exe` from this fork's
[Releases](https://github.com/egsok/Claude-Code-Usage-Monitor-hardened/releases) page.

Portable builds never update themselves. Once the dedicated package is published,
WinGet will be the only supported update channel.

## Use

Run:

```powershell
claude-code-usage-monitor
```

Once running, it will appear in your taskbar and as one or more tray icons in the notification area.

- Drag the left divider to move the taskbar widget
- On multi-monitor setups, drag the widget onto another Windows taskbar to move it to that screen
- Right-click the taskbar widget or tray icon for refresh, displayed models, update frequency, Start with Windows, reset position, language, updates, and exit
- Left-click the tray icon to toggle the taskbar widget on or off
- Enable `Start with Windows` from the right-click menu if you want it to launch automatically when you sign in

### Models

Use the right-click **Models** menu to choose what the widget displays:

- **Claude Code** is enabled by default
- **Codex** can be enabled alongside Claude Code or shown by itself
- **Antigravity** can be enabled alongside the other providers or shown by itself as its own model column

When multiple models are shown, each model has its own usage bar and matching usage text color. Antigravity prefers Google's Gemini quota summary when available and falls back to model quota data when needed.

### System Tray Icon

The tray icon shows your current 5-hour usage as a percentage badge.

If multiple providers are enabled, the app shows one tray icon per provider. If only one model is enabled, it shows one tray icon.

The Claude Code tray icon uses the same warm usage colors as the Claude bar. The Codex tray icon uses a black and white badge style. The Antigravity tray icon uses a blue badge style.

Hovering over a tray icon shows the usage values for that model.

## Diagnostics

If you need to troubleshoot startup or visibility issues, run:

```powershell
claude-code-usage-monitor --diagnose
```

This writes a log file to:

```text
%TEMP%\claude-code-usage-monitor.log
```

Settings are saved to:

```text
%APPDATA%\ClaudeCodeUsageMonitor\settings.json
```

## Account Support

This app works with the same account types that Claude Code itself supports.

As of **March 19, 2026**, Anthropic's Claude Code setup documentation says:

- **Supported:** Pro, Max, Teams, Enterprise, and Console accounts
- **Not supported:** the free Claude.ai plan

If Anthropic changes Claude Code availability in the future, this app should follow whatever Claude Code supports, as long as the usage data remains exposed through the same authenticated endpoints.

## Privacy And Security

This project is **open source**, so you can inspect exactly what it does.

What the app reads:

- Your local Claude Code OAuth credentials from `~/.claude/.credentials.json`
- If needed, the same credentials file inside an installed WSL distro
- If Codex is enabled, your local Codex credentials from `$CODEX_HOME/auth.json` or `~/.codex/auth.json`
- If Antigravity is enabled, your local Antigravity OAuth token from Windows Credential Manager target `gemini:antigravity`

What the app sends over the network:

- Requests to Anthropic's Claude endpoints to read your usage and rate-limit information
- Requests to ChatGPT's Codex usage endpoint to read your Codex usage and rate-limit information, if Codex is enabled
- Requests to Google's Cloud Code / Antigravity endpoints to read your Antigravity quota information, if Antigravity is enabled
- Requests to GitHub only when the app checks this fork for a newer release
- If proxy environment variables such as `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY` are set, those outbound requests may use that proxy

What the app stores locally:

- Widget position
- Selected taskbar / screen
- Widget visibility
- Polling frequency
- Language preference
- Last update check time
- Displayed model preferences

What it does **not** do:

- It does not send your credentials to any other server
- It does not use a separate backend service
- It does not collect analytics or telemetry
- It does not upload your project files
- It does not directly edit your Codex credentials file

Notes:

- If your Claude Code or Codex token is expired, the app reports an authentication error and waits for you to log in manually
- The app never launches Claude Code or Codex CLI commands in the background
- If your Antigravity token is expired, open Antigravity and sign in again. The monitor does not write Windows Credential Manager entries itself.
- Portable installs never download or replace executable files
- Proxies should be trusted because proxied usage requests include your OAuth bearer token inside the TLS connection

## How It Works

The monitor:

1. Finds your enabled model login credentials
2. Reads your current usage from Anthropic, ChatGPT, and/or Google's Antigravity endpoints
3. Shows the result directly in the Windows taskbar
4. Keeps the widget aligned with the selected taskbar and tray area
5. Refreshes periodically in the background

If the newer usage endpoint is unavailable, it can fall back to reading the rate-limit headers returned by Claude's Messages API.

## Open Source

This project is licensed under MIT.

If you want to inspect the behavior or audit the code, everything is in this repository.

## Русский

Это security-focused fork проекта
[CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor) —
полезного нативного Windows-виджета, который показывает лимиты Claude Code,
Codex и Google Antigravity прямо в панели задач. Hardened-версия сохраняет эту
функциональность, но сужает полномочия фоновой утилиты.

### Почему появился этот fork

При проверке исходного кода мы обнаружили два удобных, но нежелательных для
пассивного монитора поведения:

1. При истечении авторизации исходная версия могла сама запускать команды Claude
   Code или Codex CLI в унаследованной рабочей директории, чтобы обновить токен.
   Агентские CLI могут загружать локальные инструкции, конфигурацию и hooks
   проекта, поэтому такой запуск способен делать больше, чем простое чтение
   статистики использования.
2. Portable self-updater мог скачать исполняемый файл из последнего GitHub
   Release и заменить текущий `.exe` без отдельной проверки цифровой подписи или
   контрольной суммы.

Это выглядело как реализация функций для удобства, а не как злонамеренное
поведение. Hardened-fork просто использует более строгую модель доверия.

### Что изменено

- Приложение никогда не запускает Claude Code или Codex CLI. Если авторизация
  истекла, нужно самостоятельно войти в соответствующий CLI.
- Portable-версия не скачивает и не заменяет собственный `.exe`.
- Проверка GitHub Releases только сообщает о новой версии. После публикации
  отдельного пакета автоматические обновления будут выполняться только через
  WinGet.
- Добавлен регрессионный тест, запрещающий возвращение фонового запуска агентов.
- Репозиторий и будущий WinGet package ID отделены от оригинального проекта.

Утилите по-прежнему требуется читать локальные OAuth-данные провайдеров и
отправлять их на официальные endpoints статистики. Полный перечень читаемых,
отправляемых и сохраняемых данных приведён в разделе
[Privacy And Security](#privacy-and-security).

Для запуска нужны Windows 10/11 и уже авторизованный Claude Code. Поддержка
Codex и Google Antigravity включается опционально. Если токен истёк, сначала
войдите в соответствующий CLI вручную, а затем запустите монитор.
