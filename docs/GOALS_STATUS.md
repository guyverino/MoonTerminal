# Goals Status

Дата актуализации: 2026-06-18.

## Главные критерии

1. График сам решает, нужен ли кадр; если решил рисовать — данные готовятся и рисуются в том же
   кадре, без пропуска на следующий tick.
2. Нет бездумной очистки/present на каждый такт монитора и нет top-down перерисовки Shell/Orders
   из-за live-scroll или mousemove чарта.
3. Решение кроссплатформенное: Windows, macOS, Linux.
4. Реализация минимальная и строгая: без скрытых таймеров, без ownership-ловушек, без публичных
   документов, которые описывают уже отвергнутый путь.

## Кодовый Статус

- Терминал переведён со старого ZedFork/MoonPalette пути на standalone `Moonbot-Tech/MoonUI`.
- Чарт использует `gpu_canvas.frame()` как единую точку: подтянуть видимые market data,
  принять решение о кадре и, если кадр нужен, отрисовать его в том же platform tick.
- Старый перенос живой market data через `snapshot -> FeedMsg -> SessionManager entity -> chart copy`
  снят с live-path. График тянет `snapshot_versioned()` через `MarketDataSource` внутри frame path;
  `SharedMarketStore` остаётся core-owned совместимым read-model, а не GPUI-trigger транспорта.
- Старый 8ms polling удалён. `MoonEventSink` будит backend-loop через waker; live-loop ждёт событие,
  без периодического timeout-wake.
- Shell/Orders не должны перерисовываться на chart-present rate. Диагностика:
  `MOON_RENDER_DIAG=1` и `render_diag.log`.
- Windows/macOS/Linux backend paths для chartdx существуют. Wayland имеет `gpu_canvas_frame_timer`
  для canvas-tick при Skip без грязного GPUI view.
- Present rate для phase-clean scale стартует с платформенной/bootstrap оценки, затем chartdx уточняет
  его по фактическому cadence `gpu_canvas.frame()` без нового top-down render.
- Локальная разработка MoonUI делается через ignored `[patch]` в `.cargo/config.toml`, без правки
  публичных `Cargo.toml`.

## Проверка Перед Публикацией

Кодовые цели закрыты локальной реализацией. Оставшийся этап — физическая валидация на целевых
платформах, потому что Windows, macOS, Linux X11 и Linux Wayland отличаются пейсерами, драйверами,
композиторами и secure-storage окружением. Протокол проверки: `docs/MAC_LINUX_PERF_TEST_TZ.md`.
