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

  /** Классы, которыми помечаются все найденные фрагменты и текущий из них. */
  const SEARCH_HIT_CLASS = 'search-hit';
  const SEARCH_CURRENT_CLASS = 'search-current';

  /** Теги, текст внутри которых заведомо не является содержимым документа. */
  const SEARCH_SKIP_TAGS = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEXTAREA']);

  /** Панель поиска открыта (Ctrl+F). */
  let searchOpen = false;

  /** Строка поиска. */
  let query = '';

  /** Учитывать регистр при поиске. */
  let caseSensitive = false;

  /** Найденные <mark>-элементы в порядке документа. */
  let hits = [];

  /** Индекс текущего совпадения в hits; -1 — совпадений нет. */
  let currentHit = -1;

  /** Подпись счётчика совпадений в панели поиска. */
  let hitStatus = '';

  /** Корневой элемент документа — граница поиска (bind:this). */
  let documentRoot;

  /** Поле ввода панели поиска (bind:this). */
  let searchInput;

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
      // Документ перерисован из html — подсветка поиска потеряна, повторяем поиск.
      if (searchOpen && query) runSearch();
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

  // Глобальные сочетания поиска: Ctrl+F (Cmd+F) открывает панель, Esc — закрывает.
  // Слушатель на window, чтобы поиск работал из любого места документа.
  onMount(() => {
    window.addEventListener('keydown', onGlobalKeydown);
    return () => window.removeEventListener('keydown', onGlobalKeydown);
  });

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

  /** Подпись счётчика: пусто без строки, иначе «номер / всего» либо «Нет совпадений». */
  function formatHitStatus(rawQuery, total, index) {
    if (!rawQuery) return '';
    if (total === 0) return 'Нет совпадений';
    return `${index + 1} / ${total}`;
  }

  // Счётчик пересчитывается при смене строки, числа совпадений и текущего индекса.
  $: hitStatus = formatHitStatus(query, hits.length, currentHit);

  /** Экранирует спецсимволы регулярного выражения: строка поиска — литерал. */
  function escapeRegExp(text) {
    return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  /** Готовит регулярное выражение поиска; null — литерал не скомпилировался. */
  function buildSearchRegex(text, matchCase) {
    try {
      return new RegExp(escapeRegExp(text), matchCase ? 'gu' : 'giu');
    } catch {
      // Экзотическая строка (например, одинокий суррогат) не должна ронять поиск.
      return null;
    }
  }

  /** Собирает текстовые узлы документа, пропуская служебные теги и пустой текст. */
  function collectSearchableTextNodes(root) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode(node) {
        if (!node.nodeValue || !node.nodeValue.trim()) return NodeFilter.FILTER_REJECT;
        if (SEARCH_SKIP_TAGS.has(node.parentElement?.tagName)) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    return nodes;
  }

  /** Обёртывает все вхождения regex в тексте узла элементами <mark>.
   *  Совпадение ищется в пределах одного текстового узла: фраза, разбитая
   *  подсветкой синтаксиса на несколько <span>, целиком не найдётся. */
  function markMatchesInTextNode(node, regex) {
    const text = node.nodeValue;
    const ranges = [];
    regex.lastIndex = 0;
    let match;
    while ((match = regex.exec(text)) !== null) {
      if (match[0].length === 0) {
        regex.lastIndex += 1;
        continue;
      }
      ranges.push([match.index, match.index + match[0].length]);
    }
    if (ranges.length === 0) return;

    const fragment = document.createDocumentFragment();
    let cursor = 0;
    for (const [start, end] of ranges) {
      if (start > cursor) fragment.appendChild(document.createTextNode(text.slice(cursor, start)));
      const mark = document.createElement('mark');
      mark.className = SEARCH_HIT_CLASS;
      mark.textContent = text.slice(start, end);
      fragment.appendChild(mark);
      cursor = end;
    }
    if (cursor < text.length) fragment.appendChild(document.createTextNode(text.slice(cursor)));
    node.parentNode.replaceChild(fragment, node);
  }

  /** Убирает подсветку: <mark> заменяется исходным текстом, соседние узлы склеиваются,
   *  поэтому DOM возвращается к виду, который отдал comrak. */
  function clearHighlightMarks() {
    if (!documentRoot) return;
    const marks = documentRoot.querySelectorAll(`mark.${SEARCH_HIT_CLASS}`);
    if (marks.length === 0) return;
    const parents = new Set();
    for (const mark of marks) {
      const parent = mark.parentNode;
      if (!parent) continue;
      parents.add(parent);
      parent.replaceChild(document.createTextNode(mark.textContent), mark);
    }
    parents.forEach((parent) => parent.normalize());
  }

  /** Подсвечивает текущее совпадение и подматывает контейнер документа к нему. */
  function showCurrentHit() {
    hits.forEach((mark, index) => {
      mark.classList.toggle(SEARCH_CURRENT_CLASS, index === currentHit);
    });
    const current = hits[currentHit];
    if (current) current.scrollIntoView({ block: 'center' });
  }

  /** Переход к следующему (delta=1) или предыдущему (delta=-1) совпадению по кругу. */
  function stepSearch(delta) {
    if (hits.length === 0) return;
    currentHit = (currentHit + delta + hits.length) % hits.length;
    showCurrentHit();
  }

  /** Выбирает стартовое совпадение: первое видимое в окне или ближайшее ниже него
   *  (если ниже ничего нет — первое в документе), чтобы поиск не прыгал в начало. */
  function pickInitialHit() {
    const viewport = documentRoot.closest('.content');
    if (!viewport || hits.length === 0) return 0;
    const viewTop = viewport.getBoundingClientRect().top;
    const index = hits.findIndex((hit) => hit.getBoundingClientRect().bottom >= viewTop);
    return index === -1 ? 0 : index;
  }

  /** Пересобирает подсветку под текущую строку и переходит к стартовому совпадению. */
  function runSearch() {
    clearHighlightMarks();
    hits = [];
    currentHit = -1;
    if (!query || !documentRoot) return;
    const regex = buildSearchRegex(query, caseSensitive);
    if (!regex) return;
    for (const node of collectSearchableTextNodes(documentRoot)) {
      markMatchesInTextNode(node, regex);
    }
    hits = Array.from(documentRoot.querySelectorAll(`mark.${SEARCH_HIT_CLASS}`));
    currentHit = hits.length > 0 ? pickInitialHit() : -1;
    if (currentHit >= 0) showCurrentHit();
  }

  /** Ввод в поле поиска: обновляет строку и сразу пересобирает подсветку. */
  function onSearchInput(event) {
    query = event.currentTarget.value;
    runSearch();
  }

  /** Enter/Shift+Enter — следующее/предыдущее совпадение, Esc — закрыть панель. */
  function onSearchKeydown(event) {
    if (event.key === 'Enter') {
      event.preventDefault();
      stepSearch(event.shiftKey ? -1 : 1);
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      closeSearch();
    }
  }

  /** Переключает учёт регистра и повторяет поиск по уже введённой строке. */
  function toggleCaseSensitive() {
    caseSensitive = !caseSensitive;
    if (query) runSearch();
    focusSearchInput();
  }

  /** Возвращает фокус в поле поиска (после клика по кнопкам панели). */
  function focusSearchInput() {
    searchInput?.focus();
  }

  /** Открывает панель поиска и ставит фокус в поле (Ctrl+F). */
  async function openSearch() {
    searchOpen = true;
    await tick();
    if (!searchInput) return;
    searchInput.focus();
    searchInput.select();
  }

  /** Закрывает панель и снимает подсветку; документ остаётся на текущем совпадении. */
  function closeSearch() {
    if (!searchOpen) return;
    clearHighlightMarks();
    searchOpen = false;
    query = '';
    hits = [];
    currentHit = -1;
  }

  /** Горячие клавиши: Ctrl+F (Cmd+F) открывает поиск, Esc закрывает панель. */
  function onGlobalKeydown(event) {
    const findShortcut =
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      (event.code === 'KeyF' || event.key.toLowerCase() === 'f');
    if (findShortcut) {
      event.preventDefault();
      openSearch();
      return;
    }
    if (event.key === 'Escape' && searchOpen) {
      event.preventDefault();
      closeSearch();
    }
  }
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

  {#if searchOpen}
    <div class="findbar" role="search" aria-label="Поиск по документу">
      <svg class="findbar-icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
        <circle cx="6" cy="6" r="4.25" fill="none" stroke="currentColor" stroke-width="1.4" />
        <path d="M9.3 9.3 L12.6 12.6" stroke="currentColor" stroke-width="1.4" fill="none" />
      </svg>
      <input
        bind:this={searchInput}
        class="findbar-input"
        type="text"
        placeholder="Найти в документе"
        spellcheck="false"
        autocomplete="off"
        aria-label="Строка поиска по документу"
        oninput={onSearchInput}
        onkeydown={onSearchKeydown}
      />
      <span class="findbar-count" aria-live="polite">{hitStatus}</span>
      <button
        class="findbar-btn"
        class:active={caseSensitive}
        title="Учитывать регистр"
        aria-label="Учитывать регистр"
        aria-pressed={caseSensitive}
        onclick={toggleCaseSensitive}
      >Aa</button>
      <button
        class="findbar-btn"
        title="Предыдущее совпадение (Shift+Enter)"
        aria-label="Предыдущее совпадение"
        disabled={hits.length === 0}
        onclick={() => { stepSearch(-1); focusSearchInput(); }}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M5 1 L1.5 6 H8.5 Z" fill="currentColor" />
        </svg>
      </button>
      <button
        class="findbar-btn"
        title="Следующее совпадение (Enter)"
        aria-label="Следующее совпадение"
        disabled={hits.length === 0}
        onclick={() => { stepSearch(1); focusSearchInput(); }}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M5 9 L1.5 4 H8.5 Z" fill="currentColor" />
        </svg>
      </button>
      <button
        class="findbar-btn"
        title="Закрыть (Esc)"
        aria-label="Закрыть поиск"
        onclick={closeSearch}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M1 1 L9 9 M9 1 L1 9" stroke="currentColor" stroke-width="1.5" fill="none" />
        </svg>
      </button>
    </div>
  {/if}

  <!-- .content — полноширинный прокручиваемый контейнер: полоса прокрутки
       всегда у края окна, а не рядом с центрированной колонкой текста. -->
  <div class="content">
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
    <main class="markdown-body" bind:this={documentRoot} onclick={onContentClick}>
      {#if error}
        <div class="notice">{error}</div>
      {:else}
        {@html html}
      {/if}
    </main>
  </div>
</div>
