<script>
  import { onMount, onDestroy, tick } from 'svelte';
  import { invoke, convertFileSrc } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { highlightMarkdown } from './highlight.js';

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

  /** Пауза показа результата действия, мс. */
  const RESULT_MS = 1500;

  /** Текущее состояние кнопки копирования HTML-разметки. */
  let copyState = CopyState.Idle;

  /** Таймеры сброса подписей кнопок после показа результата. */
  const feedbackTimers = { copy: null, save: null };

  /** Показывает временный результат действия и через паузу возвращает исходный. */
  function showFeedback(key, reset) {
    clearTimeout(feedbackTimers[key]);
    feedbackTimers[key] = setTimeout(() => {
      feedbackTimers[key] = null;
      reset();
    }, RESULT_MS);
  }

  /** Состояния кнопки сохранения. */
  const SaveState = {
    Idle: 'idle',
    Busy: 'busy',
    Saved: 'saved',
    Error: 'error',
  };

  /** Подписи кнопки сохранения по состояниям: в busy показывается подпись idle. */
  const SAVE_LABELS = {
    [SaveState.Idle]: 'Сохранить',
    [SaveState.Busy]: 'Сохранить',
    [SaveState.Saved]: 'Сохранено',
    [SaveState.Error]: 'Не сохранено',
  };

  /** Текущее состояние кнопки сохранения. */
  let saveState = SaveState.Idle;

  /** Открыт текстовый редактор вместо просмотра. */
  let editMode = false;

  /** Текст в редакторе; расходится с source, пока правки не сохранены. */
  let editText = '';

  /** HTML подсветки Markdown для оверлея под прозрачной textarea. */
  let highlighted = '';

  /** Исходник файла, соответствующий отрисованному HTML. */
  let source = '';

  /** Ревизия содержимого файла — для обнаружения внешних изменений. */
  let revision = '';

  /** Несохранённые правки в редакторе. */
  let dirty = false;

  /** На диске появилась другая версия, но в редакторе есть несохранённые правки. */
  let externalChange = false;

  /** Прокрутка просмотра до входа в редактор: возвращаем её при выходе. */
  let previewScrollTop = 0;

  /** Позиция каретки в редакторе, сохраняемая при переключении режимов. */
  let lastEditorSelection = { start: 0, end: 0 };

  /** Прокрутка редактора до выхода в просмотр — возвращаем её при следующем входе. */
  let lastEditorScroll = 0;

  /** Количество строк в редакторе для статусной строки. */
  let editLineCount = 0;

  /** Подпись статусной строки: строки и символы с русскими формами plural. */
  let editStats = '';

  /** Контейнер документа (bind:this) — его прокрутка сохраняется при обновлении. */
  let contentEl;

  /** Поле редактора (bind:this). */
  let editorInput;

  /** Оверлей подсветки редактора (bind:this). */
  let editorHighlightEl;

  /** Таймер опроса файла на внешние изменения. */
  let watchTimer = null;

  /** Опрос файла уже выполняется — второй параллельный вызов не нужен. */
  let polling = false;

  /** Загрузка документа уже выполняется. */
  let loading = false;

  /** Компонент ещё смонтирован: после onDestroy таймер не перезапускается. */
  let alive = true;

  /** Период опроса файла на внешние изменения, мс. */
  const WATCH_INTERVAL_MS = 800;

  // dirty и счётчик строк зависят от editText и исходника; пересчитываются
  // после каждой вставки/загрузки. Для проверок в обработчиках сравниваются
  // сами строки: реактивное dirty обновляется только на следующем тике.
  $: dirty = editMode && editText !== source;
  $: editLineCount = editText ? editText.split('\n').length : 0;
  $: editStats = `${formatPlural(editLineCount, ['строка', 'строки', 'строк'])} · ${formatPlural(editText.length, ['символ', 'символа', 'символов'])}`;

  /** Русская форма существительного для числа: 1 строка, 2 строки, 5 строк. */
  function formatPlural(count, forms) {
    const tail = count % 100;
    if (tail >= 11 && tail <= 14) return `${count} ${forms[2]}`;
    const last = count % 10;
    if (last === 1) return `${count} ${forms[0]}`;
    if (last >= 2 && last <= 4) return `${count} ${forms[1]}`;
    return `${count} ${forms[2]}`;
  }

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

  /** Загружает документ из Rust. silent — не показывать ошибку (фоновое обновление),
   *  keepScroll — вернуть прокрутку на прежнее место, force — перезаписать
   *  содержимое редактора даже при несохранённых правках. */
  async function load({ keepScroll = false, silent = false, force = false } = {}) {
    if (loading) return;
    loading = true;

    const viewScroll = contentEl ? contentEl.scrollTop : 0;
    const editorState = captureEditorState();
    // Сравниваем с исходником ДО применения нового документа: иначе внешнее
    // изменение выглядело бы как несохранённая правка пользователя.
    const previousSource = source;

    if (!silent) {
      ready = false;
      error = null;
    }

    try {
      const doc = await invoke('read_markdown');
      applyDocument(doc);
      ready = true;
      error = null;

      // В редакторе без несохранённых правок показываем актуальный исходник.
      // Несохранённые правки не затираем: это защита от потери текста.
      const hasUnsavedEdits = editText !== previousSource;
      if (editMode && (force || !hasUnsavedEdits)) {
        editText = source;
        externalChange = false;
        highlighted = highlightMarkdown(editText);
      } else if (editMode) {
        externalChange = true;
      }

      // Обновляем настоящий title окна (виден в таск-менеджерах и WM);
      // сбой не должен мешать отображению документа.
      appWindow.setTitle(name).catch(() => {});
      document.title = name;

      await tick();
      rewriteImageSrcs();
      // Возвращаем прокрутку до пересчёта поиска: pickInitialHit выбирает
      // совпадение по видимой области и должен видеть прежнюю позицию.
      if (keepScroll && contentEl) contentEl.scrollTop = viewScroll;
      // Документ перерисован из html — подсветка поиска потеряна, повторяем поиск
      // без подмотки: при фоновом обновлении страница остаётся на своей позиции.
      if (searchOpen && query) runSearch({ scroll: false });

      if (editMode) restoreEditorState(editorState);
    } catch (err) {
      // Фоновое обновление не должно стирать уже показанный документ:
      // ошибку показываем только при явной загрузке.
      if (!silent) {
        error = String(err);
      }
    } finally {
      loading = false;
    }
  }

  /** Применяет документ, полученный от Rust, к состоянию компонента. */
  function applyDocument(doc) {
    html = doc.html;
    source = doc.source;
    name = doc.name;
    baseDir = doc.base_dir;
    revision = doc.revision;
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

  // При монтировании компонента сразу открываем документ и запускаем опрос
  // файла на внешние изменения. При размонтировании таймер опроса снимается.
  onMount(() => {
    alive = true;
    load();
    scheduleWatch();
    return () => {
      alive = false;
      clearTimeout(watchTimer);
    };
  });

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
    showFeedback('copy', () => {
      copyState = CopyState.Idle;
    });
  }

  // Снимаем таймеры сброса подписей, чтобы коллбеки не сработали после размонтирования.
  onDestroy(() => Object.values(feedbackTimers).forEach((timer) => clearTimeout(timer)));

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
  function showCurrentHit(scroll = true) {
    hits.forEach((mark, index) => {
      mark.classList.toggle(SEARCH_CURRENT_CLASS, index === currentHit);
    });
    const current = hits[currentHit];
    if (current && scroll) current.scrollIntoView({ block: 'center' });
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

  /** Пересобирает подсветку под текущую строку и переходит к стартовому совпадению.
   *  scroll=false — не трогать прокрутку (фоновое обновление документа). */
  function runSearch({ scroll = true } = {}) {
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
    if (currentHit >= 0) showCurrentHit(scroll);
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

  /** Планирует следующий опрос файла. Таймер у каждого цикла один, поэтому
   *  параллельные вызовы не накапливаются. */
  function scheduleWatch() {
    clearTimeout(watchTimer);
    watchTimer = setTimeout(pollDocument, WATCH_INTERVAL_MS);
  }

  /** Сравнивает ревизию файла на диске с загруженной. При внешнем изменении
   *  обновляет документ с сохранением прокрутки, а в редакторе с несохранёнными
   *  правками не затирает текст и показывает предупреждение. */
  async function pollDocument() {
    if (!alive) return;
    if (!ready || polling || loading) {
      scheduleWatch();
      return;
    }
    polling = true;
    try {
      const nextRevision = await invoke('markdown_revision');
      if (nextRevision === revision) return;
      if (editMode && editText !== source) {
        externalChange = true;
        return;
      }
      // Фоновое обновление: ошибку не показываем, файл может быть занят
      // на мгновение при атомарной замене; следующий опрос повторит чтение.
      await load({ keepScroll: true, silent: true });
    } catch {
      // Недоступный файл — штатная ситуация на время записи; ждём следующий опрос.
    } finally {
      polling = false;
      if (alive) scheduleWatch();
    }
  }

  /** Возвращает прокрутку оверлея подсветки в позицию textarea. */
  function syncEditorScroll() {
    if (!editorInput || !editorHighlightEl) return;
    editorHighlightEl.scrollTop = editorInput.scrollTop;
    editorHighlightEl.scrollLeft = editorInput.scrollLeft;
  }

  /** Снимает прокрутку и выделение textarea перед обновлением содержимого. */
  function captureEditorState() {
    if (!editorInput) return { scroll: 0, start: 0, end: 0 };
    return {
      scroll: editorInput.scrollTop,
      start: editorInput.selectionStart,
      end: editorInput.selectionEnd,
    };
  }

  /** Восстанавливает прокрутку и выделение после обновления textarea. Сначала
   *  ставим каретку (браузер может подмотать к ней), затем возвращаем scrollTop. */
  function restoreEditorState(state) {
    if (!editorInput) return;
    const max = editorInput.value.length;
    editorInput.setSelectionRange(Math.min(state.start, max), Math.min(state.end, max));
    editorInput.scrollTop = state.scroll;
    syncEditorScroll();
  }

  /** Пересчитывает подсветку после изменения текста и синхронизирует прокрутку
   *  (пересоздание разметки может сбросить scrollTop оверлея). */
  function updateHighlight() {
    highlighted = highlightMarkdown(editText);
    tick().then(syncEditorScroll);
  }

  /** Ввод в редактор: обновляет текст и подсветку. */
  function onEditorInput(event) {
    editText = event.currentTarget.value;
    updateHighlight();
    syncEditorScroll();
  }

  /** Запоминает позицию каретки и прокрутку для возврата в редактор. */
  function rememberEditorSelection() {
    if (!editorInput) return;
    lastEditorSelection = { start: editorInput.selectionStart, end: editorInput.selectionEnd };
    lastEditorScroll = editorInput.scrollTop;
  }

  /** Вставляет текст в позицию каретки, сохраняя нативный undo-стек textarea. */
  function insertEditorText(text) {
    if (!editorInput) return;
    editorInput.setRangeText(text, editorInput.selectionStart, editorInput.selectionEnd, 'end');
    editText = editorInput.value;
    updateHighlight();
    syncEditorScroll();
  }

  /** Tab вставляет отступ, Ctrl+S (Cmd+S) сохраняет — как в обычном редакторе. */
  function onEditorKeydown(event) {
    if ((event.ctrlKey || event.metaKey) && !event.altKey && (event.code === 'KeyS' || event.key.toLowerCase() === 's')) {
      event.preventDefault();
      event.stopPropagation();
      saveMarkdown();
      return;
    }
    if (event.key === 'Tab' && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault();
      insertEditorText('  ');
    }
  }

  /** Показывает результат сохранения и через паузу возвращает подпись кнопки. */
  function showSaveResult(state) {
    saveState = state;
    showFeedback('save', () => {
      saveState = SaveState.Idle;
    });
  }

  /** Сохраняет текст редактора в открытый файл. При ошибке правки остаются
   *  в редакторе, а кнопка сообщает о неудаче. */
  async function saveMarkdown() {
    if (!editMode || editText === source || saveState === SaveState.Busy) return;

    saveState = SaveState.Busy;
    const editorState = captureEditorState();

    try {
      const doc = await invoke('write_markdown', { content: editText });
      applyDocument(doc);
      editText = doc.source;
      externalChange = false;
      highlighted = highlightMarkdown(editText);

      await tick();
      rewriteImageSrcs();
      restoreEditorState(editorState);
      showSaveResult(SaveState.Saved);
    } catch {
      // Содержимое редактора не трогаем: пользователь должен иметь возможность
      // повторить сохранение или скопировать текст.
      showSaveResult(SaveState.Error);
    }
  }

  /** Явная загрузка версии с диска: правки в редакторе будут потеряны. */
  async function reloadFromDisk() {
    await load({ keepScroll: true, force: true, silent: false });
  }

  /** Переключает просмотр и редактор. Выход с несохранёнными правками требует
   *  подтверждения: документ остаётся на диске в последнем сохранённом виде. */
  async function toggleEditMode() {
    if (!ready) return;

    if (editMode) {
      rememberEditorSelection();
      if (editText !== source && !window.confirm('Несохранённые изменения будут потеряны. Выйти из режима редактирования?')) {
        return;
      }
      const scroll = previewScrollTop;
      editMode = false;
      editText = source;
      externalChange = false;
      await tick();
      if (contentEl) contentEl.scrollTop = scroll;
      return;
    }

    if (searchOpen) closeSearch();
    previewScrollTop = contentEl ? contentEl.scrollTop : 0;
    editMode = true;
    editText = source;
    externalChange = false;
    highlighted = highlightMarkdown(editText);
    await tick();
    syncEditorScroll();
    if (editorInput) editorInput.focus();
    restoreEditorState({ scroll: lastEditorScroll, ...lastEditorSelection });
  }

  /** Горячие клавиши: Ctrl+F (Cmd+F) открывает поиск, Esc закрывает панель,
   *  Ctrl+S сохраняет правки в режиме редактирования. */
  function onGlobalKeydown(event) {
    if (event.defaultPrevented) return;
    const primary = event.ctrlKey || event.metaKey;

    if (primary && !event.altKey && (event.code === 'KeyS' || event.key.toLowerCase() === 's')) {
      if (editMode) {
        event.preventDefault();
        saveMarkdown();
      }
      return;
    }

    const findShortcut = primary && !event.altKey && (event.code === 'KeyF' || event.key.toLowerCase() === 'f');
    if (findShortcut && !editMode) {
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
      {#if editMode}
        <button
          class="ctl action"
          class:error={saveState === SaveState.Error}
          aria-label="Сохранить изменения"
          title="Сохранить изменения (Ctrl+S)"
          disabled={!dirty || saveState === SaveState.Busy}
          onclick={saveMarkdown}
        >
          <svg class="action-icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
            <path d="M2 1 H9.5 L13 4.5 V13 H1 V1 Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" />
            <path d="M4 1 V5 H10 V1" fill="none" stroke="currentColor" stroke-width="1.3" />
            <rect x="4" y="8" width="6" height="4" fill="none" stroke="currentColor" stroke-width="1.3" />
          </svg>
          <span class="action-label">{SAVE_LABELS[saveState]}</span>
        </button>
        <button
          class="ctl action"
          aria-label="Вернуться к просмотру"
          title="Вернуться к просмотру"
          disabled={saveState === SaveState.Busy}
          onclick={toggleEditMode}
        >
          <svg class="action-icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
            <path d="M1 7 C3.4 3.8 10.6 3.8 13 7 C10.6 10.2 3.4 10.2 1 7 Z" fill="none" stroke="currentColor" stroke-width="1.3" />
            <circle cx="7" cy="7" r="2.1" fill="none" stroke="currentColor" stroke-width="1.3" />
          </svg>
          <span class="action-label">Просмотр</span>
        </button>
      {:else}
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
        <button
          class="ctl action"
          aria-label="Редактировать Markdown"
          title="Редактировать Markdown"
          disabled={!ready}
          onclick={toggleEditMode}
        >
          <svg class="action-icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
            <path d="M1.5 12.5 L2.2 9.6 L9.6 2.2 L11.8 4.4 L4.4 11.8 Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" />
            <path d="M9.6 2.2 L11.8 4.4" fill="none" stroke="currentColor" stroke-width="1.3" />
          </svg>
          <span class="action-label">Редактировать</span>
        </button>
      {/if}
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
       всегда у края окна, а не рядом с центрированной колонкой текста.
       В режиме редактирования тот же контейнер занимает редактор. -->
  <div class="content" class:editing={editMode} bind:this={contentEl}>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
    <main class="markdown-body" class:preview-hidden={editMode} bind:this={documentRoot} onclick={onContentClick}>
      {#if error}
        <div class="notice">{error}</div>
      {:else}
        {@html html}
      {/if}
    </main>

    {#if editMode}
      <div class="editor">
        {#if externalChange}
          <div class="editor-banner" role="alert">
            <span class="editor-banner-text">Файл изменён на диске. Ваши правки не потеряны.</span>
            <button type="button" class="editor-banner-btn" onclick={reloadFromDisk}>Загрузить с диска</button>
            <button
              type="button"
              class="editor-banner-btn"
              onclick={saveMarkdown}
              disabled={saveState === SaveState.Busy}
            >Сохранить мои</button>
          </div>
        {/if}

        <!-- Текст textarea прозрачен: цветной текст рисует оверлей под ним.
             Оба слоя имеют одинаковые шрифт, отступы и перенос строк,
             поэтому каретка совпадает с подсветкой. -->
        <div class="editor-body">
          <pre class="editor-highlight" bind:this={editorHighlightEl} aria-hidden="true">{@html highlighted}</pre>
          <textarea
            class="editor-input"
            bind:this={editorInput}
            bind:value={editText}
            placeholder="Текст Markdown"
            spellcheck="false"
            autocomplete="off"
            autocapitalize="off"
            wrap="soft"
            aria-label="Исходный текст Markdown"
            oninput={onEditorInput}
            onscroll={syncEditorScroll}
            onkeydown={onEditorKeydown}
            onselect={rememberEditorSelection}
            onkeyup={rememberEditorSelection}
            onclick={rememberEditorSelection}
          ></textarea>
        </div>

        <div class="editor-status">
          <span class="editor-dirty" class:on={dirty}>{dirty ? '● не сохранено' : '✓ сохранено'}</span>
          <span>{editStats}</span>
          <span class="editor-hint">Tab — отступ, Ctrl+S — сохранить</span>
        </div>
      </div>
    {/if}
  </div>
</div>
