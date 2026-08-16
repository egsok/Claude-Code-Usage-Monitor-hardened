# Backlog

Unscheduled ideas for possible future releases. Items here are not commitments
and have no target version.

## Smart taskbar collision avoidance

Detect the occupied bounds of the centered Windows 11 taskbar icon group and
keep the widget out of that area. Prefer its saved position when it fits; when
it would overlap taskbar icons, move it to the nearest free side and fall back
to floating placement only when neither side has enough room.

Constraints:

- Do not inject code into Explorer or modify the Windows 11 XAML taskbar.
- Preserve multi-monitor, taskbar, floating, and tray-only behavior.
- Treat taskbar internals as unstable and fail safely after Windows updates.

## По-русски

В будущем можно добавить автоматическое предотвращение пересечения виджета с
центрированной группой иконок Windows 11. Виджет сохраняет выбранную позицию,
пока она свободна; при конфликте перемещается на ближайшую свободную сторону и
переходит в floating только при отсутствии места с обеих сторон. Решение не
должно внедряться в Explorer или модифицировать XAML панели задач.
