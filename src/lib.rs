use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use comrak::{
    adapters::SyntaxHighlighterAdapter,
    html::write_opening_tag,
    markdown_to_html_with_plugins, Options,
    options::{Plugins, RenderPlugins},
    plugins::syntect::SyntectAdapter,
};
use serde::Serialize;
use tauri::State;

/// Открывает URL во внешнем браузере по умолчанию в системе.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    opener::open_browser(&url).map(|_| ()).map_err(|err| format!("Не удалось открыть ссылку: {err}"))
}

/// Открывает локальный .md файл в новом экземпляре программы.
/// Относительный путь разрешается относительно директории текущего документа.
#[tauri::command]
fn open_md(state: State<AppState>, path: String) -> Result<(), String> {
    let md_path = state
        .md_path
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?
        .clone()
        .ok_or_else(|| "Не передан путь к .md файлу".to_string())?;

    let target = PathBuf::from(&path);
    let resolved = if target.is_absolute() {
        target
    } else {
        // Базой для относительных ссылок служит папка открытого документа.
        let base = md_path
            .parent()
            .ok_or_else(|| "Нет базовой директории для ссылки".to_string())?;
        base.join(&target)
    };
    let resolved = resolved
        .canonicalize()
        .map_err(|err| format!("Файл не найден: {err}"))?;

    // Запускаем новый экземпляр этого же исполняемого файла с путём к .md файлу.
    let exe = std::env::current_exe()
        .map_err(|err| format!("Не найден исполняемый файл: {err}"))?;
    std::process::Command::new(exe)
        .arg(&resolved)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("Не удалось открыть новый экземпляр: {err}"))
}

/// Состояние приложения: путь к открываемому .md файлу и снимок его исходника.
/// Mutex нужен, т.к. команды Tauri приходят из асинхронных коллбеков вебвью.
struct AppState {
    md_path: Mutex<Option<PathBuf>>,
    /// Исходник последнего успешно открытого документа — источник HTML-разметки
    /// для копирования в вики. Снимок делается в момент открытия файла, поэтому
    /// в буфер обмена попадает ровно то, что показано в окне, даже если файл
    /// на диске уже изменился.
    source: Mutex<Option<String>>,
}

/// Достаёт первый аргумент командной строки и приводит его к абсолютному пути.
/// canonicalize разрешает относительный путь и заодно проверяет существование файла.
fn first_arg_path() -> Option<PathBuf> {
    let raw: PathBuf = PathBuf::from(std::env::args().nth(1)?);
    Some(raw.canonicalize().unwrap_or(raw))
}

/// Возвращает путь открытого документа и его исходный текст.
/// Путь клонируется под блокировкой, и Mutex отпускается до чтения с диска,
/// чтобы не держать блокировку во время медленного ввода-вывода.
fn read_document(state: &AppState) -> Result<(PathBuf, String), String> {
    let md_path = state
        .md_path
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?
        .clone()
        .ok_or_else(|| "Не передан путь к .md файлу".to_string())?;

    let source = fs::read_to_string(&md_path)
        .map_err(|err| format!("Не удалось прочитать файл {}: {}", md_path.display(), err))?;

    Ok((md_path, source))
}

/// Встроенные грамматики syntect не содержат TypeScript и shell/dotenv, поэтому
/// при подсветке язык фенса подменяется ближайшей грамматикой (TS — надмножество JS,
/// shell — bash, dotenv — properties). Опции после запятой SyntectAdapter разбирает
/// сам, поэтому сохраняем их как есть; класс language-* в разметке не меняется.
fn alias_language(lang: &str) -> String {
    const ALIASES: [(&str, &str); 5] = [
        ("typescript", "js"), ("ts", "js"), ("tsx", "js"),
        ("shell", "bash"), ("dotenv", "properties"),
    ];

    let (token, suffix) = match lang.split_once(',') {
        Some((token, options)) => (token, format!(",{options}")),
        None => (lang, String::new()),
    };

    match ALIASES.iter().find(|(name, _)| token.to_lowercase() == *name) {
        Some((_, replacement)) => format!("{replacement}{suffix}"),
        None => lang.to_string(),
    }
}

/// Режим рендера блоков кода: чем задаются цвета и оформление.
#[derive(Clone, Copy)]
enum CodeRender {
    /// Экран: подсветка классами syntect, цвета и рамки — из styles.css.
    Screen,
    /// Вики: Confluence не подключает CSS и вырезает классы, поэтому цвета и оформление
    /// кладём инлайновыми стилями, а цвета подсветки — токенами Atlassian Design System.
    Wiki,
}

/// Блок кода для вики: рамка, фон и моноширинный шрифт. Фон и рамку задаём токенами
/// Atlassian Design System: Confluence заменяет литеральные цвета своими токенами,
/// а готовые токены сохраняет; в скобках — наш цвет как fallback для вики без токенов.
/// Цвет текста задаём только токеном, без своего hex: hex не переключается вместе с темой
/// Confluence, и в тёмной теме тёмный fallback нечитаем на тёмном фоне; если токен
/// не определён, объявление недействительно и текст наследует цвет темы. В font-family
/// только имена без пробелов: Confluence берёт пробельные имена в кавычки внутри style="",
/// из-за чего атрибут рвётся на части. Отступ сверху задаём самому блоку (как у цитаты):
/// в вики предыдущий элемент может не давать своего нижнего отступа, и блок липнет к нему;
/// нижний отступ оставляем — он даёт зазор перед следующим элементом.
const WIKI_PRE_STYLE: &str = "background-color:var(--ds-surface-sunken,#f6f8fa);border:1px solid var(--ds-border,#d0d7de);border-radius:6px;padding:12px 16px;overflow-x:auto;white-space:pre;line-height:1.45;color:var(--ds-text-code-default);font-family:ui-monospace,Menlo,Consolas,monospace;font-size:0.9em;margin:16px 0 16px;";

/// Внутренний <code> блока кода: сбрасываем стили вики, чтобы фон и рамку задавал
/// только <pre>, а цвета текста приходили от подсветки.
const WIKI_BLOCK_CODE_STYLE: &str = "display:block;background:none;border:0;padding:0;color:inherit;white-space:inherit;font-family:inherit;font-size:inherit;";

/// Инлайновый <code>: comrak рендерит его без атрибутов, поэтому стиль подставляем
/// после рендера — иначе инлайновый код в вики выглядит как обычный текст. Цвет текста —
/// только токеном, как у блока кода: свой hex не переключается вместе с темой Confluence.
const WIKI_INLINE_CODE_STYLE: &str = "background-color:var(--ds-background-neutral,#ececef);border-radius:4px;padding:2px 6px;color:var(--ds-text-code-default);font-family:ui-monospace,Menlo,Consolas,monospace;font-size:0.9em;";

/// Цитата для вики: полоса слева, фон и приглушённый текст — как в окне. Цвета — токенами
/// Atlassian с нашим fallback, как у блоков кода. Отступ сверху задаём самому блоку:
/// в вики предыдущий элемент может не давать своего нижнего отступа, и цитата липнет к нему.
const WIKI_BLOCKQUOTE_STYLE: &str = "margin:16px 0 0;padding:8px 16px;border-left:4px solid var(--ds-border,#d0d7de);background-color:var(--ds-surface-sunken,#f6f8fa);color:var(--ds-text-subtle,#59636e);";

/// Открывающие теги блока ```math: comrak пишет их мимо адаптера подсветки,
/// поэтому оформляем их отдельно — как обычный блок кода (в окне он выглядит так же).
const MATH_BLOCK_OPEN_TAG: &str = "<pre><code class=\"language-math\" data-math-style=\"display\">";

/// Цвета подсветки для классов syntect: слово scope → токен Confluence. Своего hex нет:
/// он не переключается вместе с темой Confluence, и в тёмной теме тёмный fallback нечитаем
/// на тёмном фоне; если токен не определён, объявление недействительно и текст наследует
/// цвет темы. Порядок проверок — как приоритет селекторов в styles.css приложения:
/// специфичные scope-классы раньше общих (variable.function красится как функция,
/// storage.type — как ключевое слово).
const CODE_COLOR_RULES: [(&str, &str); 15] = [
    ("punctuation", "--ds-text-code-operators"),
    ("comment", "--ds-text-code-comments"),
    ("string", "--ds-text-code-strings"),
    ("invalid", "--ds-text-danger"),
    ("error", "--ds-text-danger"),
    ("storage", "--ds-text-code-keywords"),
    ("keyword", "--ds-text-code-keywords"),
    ("support", "--ds-text-code-keywords"),
    ("constant", "--ds-text-code-numbers"),
    ("numeric", "--ds-text-code-numbers"),
    ("attribute-name", "--ds-text-code-accent-1"),
    ("function", "--ds-text-code-functions"),
    ("tag", "--ds-text-code-tags"),
    ("variable", "--ds-text-code-accent-1"),
    ("type", "--ds-text-code-accent-2"),
];

/// Подбирает токен цвета подсветки для строки классов syntect (атомы scope через пробел).
fn wiki_code_color(classes: &str) -> Option<&'static str> {
    CODE_COLOR_RULES
        .iter()
        .find(|(word, _)| classes.split(' ').any(|part| part == *word))
        .map(|(_, token)| *token)
}

/// Адаптер подсветки блоков кода: подменяет язык фенса на грамматику, которую знает
/// syntect, и пишет теги <pre>/<code> так, как нужно текущему режиму рендера.
struct CodeHighlighter {
    syntect: SyntectAdapter,
    render: CodeRender,
}

impl CodeHighlighter {
    /// Подсветку syntect всегда даёт классами: экранный режим красит их через styles.css,
    /// а вики получает цвета инлайном из этих классов (см. `style_wiki_spans`).
    fn new(render: CodeRender) -> Self {
        Self {
            syntect: SyntectAdapter::new(None),
            render,
        }
    }
}

impl SyntaxHighlighterAdapter for CodeHighlighter {
    fn write_highlighted(
        &self,
        output: &mut dyn fmt::Write,
        lang: Option<&str>,
        code: &str,
    ) -> fmt::Result {
        let aliased = lang.map(alias_language);
        self.syntect.write_highlighted(output, aliased.as_deref(), code)
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn fmt::Write,
        attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        match self.render {
            CodeRender::Screen => self.syntect.write_pre_tag(output, attributes),
            CodeRender::Wiki => write_styled_tag(output, "pre", attributes, WIKI_PRE_STYLE),
        }
    }

    fn write_code_tag(
        &self,
        output: &mut dyn fmt::Write,
        attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        match self.render {
            CodeRender::Screen => self.syntect.write_code_tag(output, attributes),
            CodeRender::Wiki => write_styled_tag(output, "code", attributes, WIKI_BLOCK_CODE_STYLE),
        }
    }
}

/// Пишет открывающий тег: атрибуты comrak (class language-*, data-*) плюс наш
/// инлайновый style; значения атрибутов comrak экранирует сам.
fn write_styled_tag(
    output: &mut dyn fmt::Write,
    tag: &str,
    attributes: HashMap<&'static str, Cow<'_, str>>,
    style: &str,
) -> fmt::Result {
    let pairs = attributes
        .into_iter()
        .map(|(key, value)| (key, value.into_owned()))
        .chain([("style", style.to_owned())]);
    write_opening_tag(output, tag, pairs)
}

/// Оформляет теги, которые comrak рендерит без стилей и мимо адаптера подсветки:
/// инлайновый <code>, блок ```math и <blockquote>. Содержимое блоков кода и подсвеченный
/// текст экранируют < и &, поэтому замена не задевает их.
fn style_wiki_tags(html: String) -> String {
    let html = html.replace("<code>", &format!("<code style=\"{WIKI_INLINE_CODE_STYLE}\">"));
    let html = html.replace(
        MATH_BLOCK_OPEN_TAG,
        &format!(
            "<pre style=\"{WIKI_PRE_STYLE}\"><code class=\"language-math\" data-math-style=\"display\" style=\"{WIKI_BLOCK_CODE_STYLE}\">"
        ),
    );
    html.replace("<blockquote>", &format!("<blockquote style=\"{WIKI_BLOCKQUOTE_STYLE}\">"))
}

/// Подставляет инлайновые цвета в спаны подсветки: вики не подключает CSS приложения,
/// поэтому классы syntect в ней бесполезны. Цвета — только токенами Confluence, без
/// нашего hex (см. CODE_COLOR_RULES). Спаны с class="" в разметке пишет только
/// syntect (сырой HTML из исходника comrak экранирует), поэтому разбор безопасен.
fn style_wiki_spans(html: String) -> String {
    const SPAN_OPEN: &str = "<span class=\"";

    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();

    while let Some(start) = rest.find(SPAN_OPEN) {
        let after = &rest[start + SPAN_OPEN.len()..];
        // Классы заканчиваются кавычкой class-атрибута, после неё syntect пишет только ">".
        let Some(class_end) = after.find('"') else { break };
        let Some(tag_end) = after[class_end + 1..].find('>') else { break };
        let classes = &after[..class_end];

        out.push_str(&rest[..start]);
        out.push_str(SPAN_OPEN);
        out.push_str(classes);
        out.push('"');
        if let Some(token) = wiki_code_color(classes) {
            out.push_str(" style=\"color:var(");
            out.push_str(token);
            out.push_str(")\"");
        }
        out.push('>');

        rest = &after[class_end + 1 + tag_end + 1..];
    }

    out.push_str(rest);
    out
}

/// Результат чтения файла: имя документа (для title окна), готовый HTML, базовая
/// директория (по ней фронтенд разрешает относительные пути к локальным картинкам),
/// исходник для редактора и ревизия файла для отслеживания внешних изменений.
#[derive(Serialize)]
struct MarkdownDoc {
    name: String,
    html: String,
    base_dir: String,
    source: String,
    revision: String,
}

/// Ревизия содержимого файла: FNV-1a по байтам. Хеш не зависит от mtime и его
/// разрешения, поэтому изменение фиксируется, даже если файл перезаписан
/// с той же длиной в ту же секунду. Сравнивается только внутри сессии.
fn content_revision(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Собирает документ из пути и уже прочитанного исходника: имя, HTML, базу для
/// картинок, исходник редактора и ревизию. Общий путь для чтения и записи файла.
fn build_document(md_path: &Path, source: String) -> MarkdownDoc {
    // comrak — CommonMark/GFM-совместимый парсер; рендер на стороне Rust
    // компактнее и производительнее, чем парсинг в браузере.
    let html = render_html(&source, CodeRender::Screen);

    // Имя файла для заголовка окна: последний компонент пути.
    let name = md_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Markdown".to_string());

    // База для относительных путей к картинкам: папка открытого документа.
    // md_path уже приведён к абсолютному пути (canonicalize в first_arg_path),
    // поэтому parent тоже абсолютен.
    let base_dir = md_path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let revision = content_revision(source.as_bytes());

    MarkdownDoc { name, html, base_dir, source, revision }
}

/// Настройки парсера, общие для экранного просмотра и копирования разметки:
/// GFM-расширения (таблицы, зачёркнутый текст, task-списки, автоссылки) и
/// tagfilter — фильтр неожиданных HTML-тегов из исходного markdown.
fn markdown_options() -> Options<'static> {
    let mut opts = Options::default();
    opts.extension.strikethrough = true;
    opts.extension.tagfilter = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;
    opts
}

/// Рендерит markdown в HTML-фрагмент тела документа: comrak не добавляет обёртку
/// html/body, поэтому фрагмент годится и для окна, и для вставки в вики.
/// Режим задаёт оформление кода: классы для экрана (цвета берутся из styles.css)
/// или инлайновые стили для вики (Confluence не подключает CSS и вырезает классы).
fn render_html(source: &str, render: CodeRender) -> String {
    let opts = markdown_options();
    let highlighter = CodeHighlighter::new(render);
    let plugins = Plugins {
        render: RenderPlugins {
            codefence_syntax_highlighter: Some(&highlighter),
            ..RenderPlugins::default()
        },
        ..Plugins::default()
    };

    let html = markdown_to_html_with_plugins(source, &opts, &plugins);

    match render {
        CodeRender::Screen => html,
        CodeRender::Wiki => style_wiki_spans(style_wiki_tags(html)),
    }
}

/// Читает markdown-файл по сохранённому пути и возвращает имя + готовый HTML.
#[tauri::command]
fn read_markdown(state: State<AppState>) -> Result<MarkdownDoc, String> {
    let (md_path, source) = read_document(&state)?;
    let doc = build_document(&md_path, source);

    // Снимок исходника: из него рендерится разметка для вики — она должна
    // совпадать с показанным документом, даже если файл на диске изменился.
    let mut snapshot = state
        .source
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?;
    *snapshot = Some(doc.source.clone());

    Ok(doc)
}

/// Возвращает ревизию содержимого файла на диске. Фронтенд опрашивает её,
/// чтобы заметить внешнее изменение открытого документа.
#[tauri::command]
fn markdown_revision(state: State<AppState>) -> Result<String, String> {
    let md_path = state
        .md_path
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?
        .clone()
        .ok_or_else(|| "Не передан путь к .md файлу".to_string())?;

    let bytes = fs::read(&md_path)
        .map_err(|err| format!("Не удалось прочитать файл {}: {}", md_path.display(), err))?;
    Ok(content_revision(&bytes))
}

/// Сохраняет текст редактора в открытый файл и возвращает уже перерендеренный
/// документ, чтобы окно сразу показало новое содержимое после выхода из редактора.
#[tauri::command]
fn write_markdown(state: State<AppState>, content: String) -> Result<MarkdownDoc, String> {
    let md_path = state
        .md_path
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?
        .clone()
        .ok_or_else(|| "Не передан путь к .md файлу".to_string())?;

    fs::write(&md_path, content.as_bytes())
        .map_err(|err| format!("Не удалось сохранить файл {}: {}", md_path.display(), err))?;

    let doc = build_document(&md_path, content);

    // Снимок для копирования HTML-разметки должен соответствовать
    // только что сохранённому содержимому.
    let mut snapshot = state
        .source
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?;
    *snapshot = Some(doc.source.clone());

    Ok(doc)
}

/// Копирует в системный буфер обмена HTML-разметку тела документа — формат,
/// который принимают вики, отображающие страницы как HTML-разметку.
#[tauri::command]
fn copy_markdown_html(state: State<AppState>) -> Result<(), String> {
    // Разметку рендерим из снимка открытого документа, а не с диска: в буфер
    // должно попасть ровно то, что показано в окне.
    let snapshot = state
        .source
        .lock()
        .map_err(|_| "Ошибка блокировки внутреннего состояния".to_string())?;
    let source = snapshot
        .as_ref()
        .ok_or_else(|| "Документ ещё не открыт".to_string())?;

    // Оформление кода — инлайновыми стилями: вики не подключает CSS просмотрщика
    // и не применяет классы, поэтому всё оформление живёт в style="".
    set_clipboard_text(&render_html(source, CodeRender::Wiki))
}

/// Кладёт текст в системный буфер обмена через GTK. Синхронные команды Tauri
/// выполняются в главном потоке GTK (обработчик IPC работает в главном цикле),
/// поэтому обращение к GTK API безопасно.
fn set_clipboard_text(text: &str) -> Result<(), String> {
    let display = gtk::gdk::Display::default()
        .ok_or_else(|| "Не найден дисплей GTK".to_string())?;
    let clipboard = gtk::Clipboard::default(&display)
        .ok_or_else(|| "Не удалось получить буфер обмена GTK".to_string())?;

    clipboard.set_text(text);
    // Просим clipboard manager сохранить содержимое: без этого на X11 текст
    // теряется после закрытия приложения (на Wayland — no-op).
    clipboard.store();

    Ok(())
}

/// Точка входа приложения: регистрирует состояние и команды Tauri.
pub fn run() {
    let md_path = first_arg_path();

    tauri::Builder::default()
        .manage(AppState {
            md_path: Mutex::new(md_path),
            source: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            read_markdown,
            markdown_revision,
            write_markdown,
            copy_markdown_html,
            open_url,
            open_md
        ])
        .run(tauri::generate_context!())
        .expect("Ошибка запуска tauri-приложения");
}
