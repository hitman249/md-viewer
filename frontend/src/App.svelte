<script>
  import { onMount, onDestroy, tick } from 'svelte';
  import { invoke, convertFileSrc } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  // Окно без системного декора: системная рамка на этой системе рисуется оконным
  // менеджером и не перекрашивается из приложения, поэтому кнопки и рамку рисуем сами.
  // Для работы команд окна (minimize/toggleMaximize/close/startDragging/setTitle) в
  // capabilities добавлены соответствующие permissions — без них вызовы молча отклонялись бы.
  const appWindow = getCurrentWindow();

  /** Готовый HTML, полученный после рендера markdown на стороне Rust. */
  let html = '';

  /** Имя открытого файла — выводится в title окна. */
  let name = 'Markdown Viewer';

  /** Текст ошибки, если файл не удалось прочитать или распарсить. */
  let error = null;

  /** Абсолютный путь к папке открытого документа — база для относительных путей картинок. */
  let baseDir = '';

  /** Документ прочитан: кнопка копирования HTML-разметки доступна. */
  let ready = false;

  /** Состояния кнопки копирования HTML-разметки. */
  const CopyState = {
    Idle: 'idle',
    Busy: 'busy',
    Copied: 'copied',
    Error: 'error',
  };

  /** Подписи кнопки копирования по состояниям: в busy показывается подпись idle. */
  const COPY_LABELS = {
    [CopyState.Idle]: 'Копировать HTML',
    [CopyState.Busy]: 'Копировать HTML',
    [CopyState.Copied]: 'Скопировано',
    [CopyState.Error]: 'Не скопировано',
  };

  /** Пауза показа результата копирования, мс. */
  const COPY_RESULT_MS = 1500;

  /** Текущее состояние кнопки копирования HTML-разметки. */
  let copyState = CopyState.Idle;

  /** Таймер сброса подписи кнопки после показа результата. */
  let copyResetTimer = null;

  /** Запрашивает у Rust-бэкенда содержимое файла, отрендеренное в HTML. */
  async function load() {
    ready = false;
    try {
      const doc = await invoke('read_markdown');
      html = doc.html;
      name = doc.name;
      baseDir = doc.base_dir;
      ready = true;
      // Обновляем настоящий title окна (виден в таск-менеджерах и WM);
      // сбой не должен мешать отображению документа.
      appWindow.setTitle(name).catch(() => {});
      document.title = name;
      // Дожидаемся, пока Svelte вставит отрендеренный HTML в DOM, затем
      // переписываем src локальных картинок на asset-URL.
      await tick();
      rewriteImageSrcs();
    } catch (err) {
      error = String(err);
    }
  }

  /** Резолвит относительный путь к картинке в абсолютный относительно baseDir. */
  function resolveImagePath(rel) {
    if (rel.startsWith('/')) return rel;
    const out = baseDir.split('/').slice();
    for (const part of rel.split('/')) {
      if (!part || part === '.') continue;
      if (part === '..') out.pop();
      else out.push(part);
    }
    return out.join('/');
  }

  /** Переписывает src локальных <img>, чтобы они грузились через asset-протокол Tauri.
   *  Относительные пути comrak оставляет как есть — они резолвятся относительно
   *  origin вебвью (tauri://localhost), а не папки документа, поэтому без этой
   *  правки картинки не отображаются. Внешние/встроенные схемы не трогаем. */
  function rewriteImageSrcs() {
    document.querySelectorAll('.markdown-body img').forEach((img) => {
      const src = img.getAttribute('src') || '';
      if (!src) return;
      if (/^(https?|data|asset|file|ftp|mailto):/i.test(src)) return;
      let raw;
      try {
        // src в HTML уже процент-закодирован comrak'ом; восстанавливаем исходный путь.
        raw = decodeURIComponent(src);
      } catch {
        raw = src;
      }
      img.src = convertFileSrc(resolveImagePath(raw));
    });
  }

  // При монтировании компонента сразу открываем документ.
  onMount(load);

  // Обработка кликов по ссылкам:
  // - http/https — открываем во внешнем браузере по умолчанию в системе;
  // - локальный путь (в т.ч. относительный .md) — открываем в новом экземпляре программы;
  // - прочие схемы (mailto:, tel:) — отдаём системному обработчику.
  function onContentClick(event) {
    const a = event.target.closest('a');
    if (!a) return;
    const href = a.getAttribute('href') || '';
    event.preventDefault();
    if (!href || href.startsWith('#')) return;
    if (/^https?:\/\//i.test(href)) {
      invoke('open_url', { url: href }).catch(() => {});
      return;
    }
    if (/^[a-z]+:/i.test(href)) {
      invoke('open_url', { url: href }).catch(() => {});
      return;
    }
    invoke('open_md', { path: href }).catch(() => {});
  }

  // Перетаскивание безрамного окна: ручной startDragging (на Linux data-tauri-drag-region
  // не работает и перехватывает клики). Кнопки управления не запускают перетаскивание.
  function onTitlebarMouseDown(event) {
    if (event.target.closest('.ctl')) return;
    if (event.button === 0) {
      appWindow.startDragging();
    }
  }

  /** Копирует в буфер обмена HTML-разметку тела документа для вставки в вики.
   *  Разметку рендерит Rust из снимка открытого файла, поэтому в буфере
   *  оказывается ровно то, что показано в окне. */
  async function copyHtml() {
    if (copyState === CopyState.Busy) return;
    copyState = CopyState.Busy;
    try {
      await invoke('copy_markdown_html');
      showCopyResult(CopyState.Copied);
    } catch {
      // Текст ошибки пользователю не нужен: кнопка сама сообщает о неудаче.
      showCopyResult(CopyState.Error);
    }
  }

  /** Показывает результат копирования и через паузу возвращает кнопку в исходный вид. */
  function showCopyResult(state) {
    copyState = state;
    clearTimeout(copyResetTimer);
    copyResetTimer = setTimeout(() => {
      copyState = CopyState.Idle;
      copyResetTimer = null;
    }, COPY_RESULT_MS);
  }

  // Снимаем таймер сброса подписи, чтобы коллбек не сработал после размонтирования.
  onDestroy(() => clearTimeout(copyResetTimer));
</script>

<div class="shell">
  <header class="titlebar" onmousedown={onTitlebarMouseDown}>
    <span class="title">
      <img src="/icon.png" class="appicon" alt="" draggable="false" />
      <span class="title-text">{name}</span>
    </span>
    <div class="controls">
      <button
        class="ctl action"
        class:error={copyState === CopyState.Error}
        aria-label="Скопировать HTML-разметку документа"
        title="Скопировать HTML-разметку тела документа для вставки в вики"
        disabled={!ready || copyState === CopyState.Busy}
        onclick={copyHtml}
      >
        <svg class="action-icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
          <rect x="5" y="1" width="8" height="8" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.4" />
          <rect x="1" y="5" width="8" height="8" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.4" />
        </svg>
        <span class="action-label">{COPY_LABELS[copyState]}</span>
      </button>
      <button class="ctl" aria-label="Свернуть" onclick={() => appWindow.minimize()}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="0" y="4" width="10" height="2" fill="currentColor" />
        </svg>
      </button>
      <button class="ctl" aria-label="Развернуть" onclick={() => appWindow.toggleMaximize()}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="1" y="1" width="8" height="8" fill="none" stroke="currentColor" stroke-width="1.5" />
        </svg>
      </button>
      <button class="ctl close" aria-label="Закрыть" onclick={() => appWindow.close()}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M1 1 L9 9 M9 1 L1 9" stroke="currentColor" stroke-width="1.5" fill="none" />
        </svg>
      </button>
    </div>
  </header>

  <!-- .content — полноширинный прокручиваемый контейнер: полоса прокрутки
       всегда у края окна, а не рядом с центрированной колонкой текста. -->
  <div class="content">
    <main class="markdown-body" onclick={onContentClick}>
      {#if error}
        <div class="notice">{error}</div>
      {:else}
        {@html html}
      {/if}
    </main>
  </div>
</div>
