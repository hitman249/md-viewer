/**
 * Подсветка исходника Markdown для текстового редактора.
 *
 * Модуль не зависит от DOM: возвращает HTML-строку, которую Svelte кладёт
 * в оверлей под прозрачной textarea. Подсветка меняет только цвет, фон и
 * подчёркивание — начертание и размер шрифта остаются такими же, как у
 * textarea, иначе каретка разойдётся с нарисованным текстом.
 */

/** Экранирует текст для вставки в HTML (текст, не значения атрибутов). */
export function escapeHtml(text) {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/** Оборачивает уже экранированный текст в span с классом подсветки. */
function span(className, text) {
  return `<span class="${className}">${text}</span>`;
}

/**
 * Инлайновые конструкции Markdown. Порядок альтернатив задаёт приоритет:
 * код и ссылки разбираются раньше выделений, поэтому звёздочки внутри
 * `кода` и адресов не считаются разметкой.
 */
const INLINE_RE =
  /(?<fence>`+)(?<fenceBody>[\s\S]*?)\k<fence>|(?<imageBang>!)\[(?<imageAlt>[^\]\n]*)\]\((?<imageTail>[^)\n]*)\)|\[(?<linkText>[^\]\n]*)\]\((?<linkTail>[^)\n]*)\)|(?<strongMarker>\*\*|__)(?<strongBody>[\s\S]*?)\k<strongMarker>|(?<strikeMarker>~~)(?<strikeBody>[\s\S]*?)\k<strikeMarker>|(?<emMarker>\*|_)(?<emBody>[^*_\n]+?)\k<emMarker>|(?<escapeMarker>\\)(?<escaped>[^\n])|(?<angleBody><[^>\n]+>)/gu;

/** Рисует ссылку или картинку: текст и адрес красятся разными классами.
 *  Собираем из срезов исходной строки, поэтому все символы сохраняются. */
function renderLinkLike(raw, isImage) {
  const openBracket = raw.indexOf(']');
  const labelStart = isImage ? 2 : 1;
  const label = raw.slice(labelStart, openBracket);
  const tail = raw.slice(raw.indexOf('](', openBracket) + 2, raw.length - 1);
  const prefix = raw.slice(0, labelStart);
  const labelClass = isImage ? 'md-image' : 'md-link-text';

  return (
    span('md-marker', escapeHtml(prefix)) +
    span(labelClass, escapeHtml(label)) +
    span('md-marker', '](') +
    span('md-url', escapeHtml(tail)) +
    span('md-marker', ')')
  );
}

/** Разбирает одно инлайновое совпадение в подсвеченные спаны. */
function renderInlineToken(match) {
  const groups = match.groups;
  const raw = match[0];

  if (groups.fenceBody !== undefined) {
    const marker = escapeHtml(groups.fence);
    return span('md-marker', marker) + span('md-code', escapeHtml(groups.fenceBody)) + span('md-marker', marker);
  }
  if (groups.imageBang !== undefined) return renderLinkLike(raw, true);
  if (groups.linkText !== undefined) return renderLinkLike(raw, false);
  if (groups.strongBody !== undefined) {
    const marker = escapeHtml(groups.strongMarker);
    return span('md-marker', marker) + span('md-strong', escapeHtml(groups.strongBody)) + span('md-marker', marker);
  }
  if (groups.strikeBody !== undefined) {
    const marker = escapeHtml(groups.strikeMarker);
    return span('md-marker', marker) + span('md-strike', escapeHtml(groups.strikeBody)) + span('md-marker', marker);
  }
  if (groups.emBody !== undefined) {
    const marker = escapeHtml(groups.emMarker);
    return span('md-marker', marker) + span('md-em', escapeHtml(groups.emBody)) + span('md-marker', marker);
  }
  if (groups.escaped !== undefined) return span('md-escape', escapeHtml(raw));
  if (groups.angleBody !== undefined) {
    const body = groups.angleBody;
    const inner = body.slice(1, body.length - 1);
    const isUrl = /^(?:[a-z][a-z0-9+.-]*:|[^\s@]+@[^\s@]+$)/i.test(inner);
    return span('md-marker', '&lt;') + span(isUrl ? 'md-url' : 'md-html', escapeHtml(inner)) + span('md-marker', '&gt;');
  }
  return escapeHtml(raw);
}

/** Подсвечивает инлайновую разметку одной строки. */
function highlightInline(line) {
  let out = '';
  let cursor = 0;
  let match;

  INLINE_RE.lastIndex = 0;
  while ((match = INLINE_RE.exec(line)) !== null) {
    if (match[0].length === 0) {
      INLINE_RE.lastIndex += 1;
      continue;
    }
    out += escapeHtml(line.slice(cursor, match.index));
    out += renderInlineToken(match);
    cursor = match.index + match[0].length;
  }

  return out + escapeHtml(line.slice(cursor));
}

/** Маркер задачи в пункте списка: `[ ]` или `[x]`. */
const TASK_RE = /^(\[[ xX]\])/;

/** Открывающий фенс блока кода: до трёх пробелов, затем три и более ` или ~. */
function parseOpeningFence(line) {
  const match = /^( {0,3})(`{3,}|~{3,})(.*)$/.exec(line);
  if (!match) return null;
  const char = match[2][0];
  // По CommonMark info-строка бэктик-фенса не может содержать бэктик.
  if (char === '`' && match[3].includes('`')) return null;
  return { char, length: match[2].length };
}

/** Закрывающий фенс: тот же символ, не короче открывающего, без текста после. */
function isClosingFence(line, fence) {
  const trimmed = line.trim();
  if (!trimmed || trimmed[0] !== fence.char) return false;
  let run = 0;
  while (run < trimmed.length && trimmed[run] === fence.char) run += 1;
  return run >= fence.length && trimmed.slice(run) === '';
}

/** Строка фенса: отступ, маркер и info-строка (язык) красятся раздельно. */
function renderFenceLine(line) {
  const match = /^(\s*)(`+|~+)(.*)$/.exec(line);
  if (!match) return span('md-code-fence', escapeHtml(line));
  return (
    escapeHtml(match[1]) +
    span('md-code-fence', escapeHtml(match[2])) +
    span('md-code-info', escapeHtml(match[3]))
  );
}

/** Горизонтальная линейка: три и более одинаковых `-`, `*` или `_`. */
function isHorizontalRule(line) {
  return /^ {0,3}([*_-])(?:[ \t]*\1){2,}[ \t]*$/.test(line);
}

/** Строка таблицы: разбиваем по неэкранированным `|`, сохраняя все символы. */
function renderTableRow(line) {
  const cells = line.split('|');
  return cells.map((cell) => highlightInline(cell)).join(span('md-marker', '|'));
}

/** Подсветка блочной разметки строки (заголовки, списки, цитаты, таблицы). */
function highlightBlockLine(line) {
  const heading = /^( {0,3})(#{1,6})([ \t]+)(.*)$/.exec(line);
  if (heading) {
    return (
      escapeHtml(heading[1]) +
      span('md-heading-marker', heading[2]) +
      escapeHtml(heading[3]) +
      span('md-heading', highlightInline(heading[4]))
    );
  }

  if (/^( {0,3})(=+|-+)[ \t]*$/.test(line)) {
    return span('md-heading-marker', escapeHtml(line));
  }

  const quote = /^(\s*)((?:>[ \t]?)+)(.*)$/.exec(line);
  if (quote) {
    return escapeHtml(quote[1]) + span('md-quote-marker', escapeHtml(quote[2])) + span('md-quote', highlightInline(quote[3]));
  }

  if (isHorizontalRule(line)) return span('md-hr', escapeHtml(line));

  const list = /^(\s*)([-+*]|\d{1,9}[.)])([ \t]+)(.*)$/.exec(line);
  if (list) {
    const task = TASK_RE.exec(list[4]);
    const rest = task
      ? span('md-task', escapeHtml(task[1])) + highlightInline(list[4].slice(task[1].length))
      : highlightInline(list[4]);
    return escapeHtml(list[1]) + span('md-list-marker', escapeHtml(list[2])) + escapeHtml(list[3]) + rest;
  }

  const definition = /^( {0,3})\[([^\]\n]+)\]:[ \t]*(\S.*)$/.exec(line);
  if (definition) {
    const open = line.indexOf('[');
    const close = line.indexOf(']:', open);
    const after = line.slice(close + 2);
    const target = after.replace(/^\s*/, '');
    return (
      escapeHtml(line.slice(0, open)) +
      span('md-marker', '[') +
      span('md-link-text', escapeHtml(line.slice(open + 1, close))) +
      span('md-marker', ']:') +
      escapeHtml(after.slice(0, after.length - target.length)) +
      span('md-url', escapeHtml(target))
    );
  }

  if (/\|/.test(line) && !/`/.test(line)) {
    return renderTableRow(line);
  }

  return highlightInline(line);
}

/**
 * Подсветка всего исходника Markdown.
 *
 * Обход построчный: фенсы кода переключают состояние, остальные строки
 * разбираются как блочная и инлайновая разметка. В конце добавляется `\n`,
 * чтобы оверлей резервировал строку под каретку после последнего перевода
 * строки так же, как это делает textarea.
 *
 * @param {string} source исходник Markdown
 * @returns {string} HTML-фрагмент с подсветкой
 */
export function highlightMarkdown(source) {
  const lines = source.split('\n');
  const out = [];
  let fence = null;

  for (const line of lines) {
    if (fence) {
      if (isClosingFence(line, fence)) {
        out.push(renderFenceLine(line));
        fence = null;
      } else {
        out.push(span('md-code-block', escapeHtml(line)));
      }
      continue;
    }

    const opening = parseOpeningFence(line);
    if (opening) {
      out.push(renderFenceLine(line));
      fence = opening;
      continue;
    }

    out.push(highlightBlockLine(line));
  }

  return out.join('\n') + '\n';
}
