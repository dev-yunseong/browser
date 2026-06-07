//! browser-cli — thin HTTP client to the browser-daemon.
//!
//! Usage:
//!   browser-cli                          # Interactive REPL (default)
//!   browser-cli navigate <url>           # Single command
//!   browser-cli click <N>                # Click link by index
//!   browser-cli click "<text>"           # Click link by text
//!   browser-cli type <field> <value>     # Type into focused field
//!   browser-cli screenshot [<file>]      # Save screenshot
//!   browser-cli back                     # Navigate back
//!   browser-cli forward                  # Navigate forward
//!   browser-cli js <script>              # Evaluate JavaScript and print result
//!   browser-cli style [<selector>]       # Print computed CSS styles
//!   browser-cli layout                   # Print the layout tree
//!   browser-cli logs                     # Print browser console entries
//!   browser-cli tick [count]             # Advance daemon JS tasks
//!   browser-cli --port 7071 <command>    # Custom daemon port

use std::fmt;

use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

// ── API mirror types (match engine.rs public API, deserialization only) ────────

#[derive(serde::Deserialize, Debug, Clone, Default)]
struct ApiRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct ApiElement {
    #[allow(dead_code)]
    pub id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub element_type: String,
    pub text: String,
    pub href: Option<String>,
    pub rect: ApiRect,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct ApiFormControl {
    pub name: String,
    pub element_type: String,
    pub rect: ApiRect,
}

#[derive(serde::Deserialize, Debug, Clone, Default)]
struct ApiPageResponse {
    pub url: String,
    pub title: String,
    pub markdown: String,
    #[serde(default)]
    pub elements: Vec<ApiElement>,
    #[serde(default)]
    pub forms: Vec<ApiFormControl>,
    pub width: u32,
    pub height: u32,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct ApiConsoleEntry {
    pub level: String,
    pub message: String,
    pub timestamp: u64,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct ApiTickResponse {
    pub ticks: u32,
    pub worked: bool,
    pub rerendered: bool,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum ClickResult {
    Navigate { url: String },
    ScriptExecuted,
    FocusChanged { id: String },
    Nothing,
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum CliError {
    DaemonNotRunning,
    Http(String),
    Parse(String),
    Io(String),
    NotFound(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::DaemonNotRunning => write!(
                f,
                "Daemon is not running.\n\
                 Start it with: browser-daemon\n\
                 Or headless:   browser-daemon --no-gui"
            ),
            CliError::Http(msg) => write!(f, "HTTP error: {}", msg),
            CliError::Parse(msg) => write!(f, "Parse error: {}", msg),
            CliError::Io(msg) => write!(f, "IO error: {}", msg),
            CliError::NotFound(msg) => write!(f, "Not found: {}", msg),
        }
    }
}

// ── DaemonClient ──────────────────────────────────────────────────────────────

struct DaemonClient {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl DaemonClient {
    fn new(port: u16) -> Self {
        Self {
            base_url: format!("http://localhost:{}", port),
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    fn check_error(e: reqwest::Error) -> CliError {
        if e.is_connect() || e.is_timeout() {
            CliError::DaemonNotRunning
        } else {
            CliError::Http(e.to_string())
        }
    }

    fn navigate(&self, url: &str) -> Result<ApiPageResponse, CliError> {
        let resp = self
            .client
            .post(format!("{}/navigate", self.base_url))
            .json(&serde_json::json!({ "url": url }))
            .send()
            .map_err(Self::check_error)?;
        if !resp.status().is_success() {
            return Err(CliError::Http(format!(
                "navigate returned {}",
                resp.status()
            )));
        }
        // Read the raw body text first so we can include it in the error message if
        // deserialization fails (e.g. because the daemon panicked and sent garbage).
        let body = resp.text().map_err(|e| CliError::Parse(e.to_string()))?;
        serde_json::from_str::<ApiPageResponse>(&body).map_err(|e| {
            let preview = if body.len() > 200 {
                &body[..200]
            } else {
                &body
            };
            CliError::Parse(format!("{} (raw body: {})", e, preview))
        })
    }

    fn get_page(&self) -> Result<ApiPageResponse, CliError> {
        let resp = self
            .client
            .get(format!("{}/page", self.base_url))
            .send()
            .map_err(Self::check_error)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(CliError::NotFound("No page loaded".to_string()));
        }
        // Read the raw body text first so we can include it in the error message if
        // deserialization fails.
        let body = resp.text().map_err(|e| CliError::Parse(e.to_string()))?;
        serde_json::from_str::<ApiPageResponse>(&body).map_err(|e| {
            let preview = if body.len() > 200 {
                &body[..200]
            } else {
                &body
            };
            CliError::Parse(format!("{} (raw body: {})", e, preview))
        })
    }

    fn click_xy(&self, x: f32, y: f32) -> Result<Vec<ClickResult>, CliError> {
        let resp = self
            .client
            .post(format!("{}/click", self.base_url))
            .json(&serde_json::json!({ "x": x, "y": y }))
            .send()
            .map_err(Self::check_error)?;
        resp.json::<Vec<ClickResult>>()
            .map_err(|e| CliError::Parse(e.to_string()))
    }

    fn type_text(&self, text: &str) -> Result<(), CliError> {
        self.client
            .post(format!("{}/type", self.base_url))
            .json(&serde_json::json!({ "text": text }))
            .send()
            .map_err(Self::check_error)?;
        Ok(())
    }

    fn submit(&self) -> Result<(), CliError> {
        self.client
            .post(format!("{}/submit", self.base_url))
            .send()
            .map_err(Self::check_error)?;
        Ok(())
    }

    fn screenshot_bytes(&self) -> Result<Vec<u8>, CliError> {
        let resp = self
            .client
            .get(format!("{}/screenshot", self.base_url))
            .send()
            .map_err(Self::check_error)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(CliError::NotFound("No page loaded".to_string()));
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| CliError::Http(e.to_string()))
    }

    fn evaluate_js(&self, script: &str) -> Result<String, CliError> {
        let resp = self
            .client
            .post(format!("{}/js", self.base_url))
            .json(&serde_json::json!({ "script": script }))
            .send()
            .map_err(Self::check_error)?;
        let v: serde_json::Value = resp.json().map_err(|e| CliError::Parse(e.to_string()))?;
        Ok(v["result"].as_str().unwrap_or("").to_string())
    }

    /// POST /console/eval — evaluate JS in the DevTools console REPL.
    /// The result/error is echoed into the daemon console buffer (visible via GET /console).
    /// Returns `(result, error)` where exactly one is `Some`.
    fn console_eval(&self, code: &str) -> Result<(Option<String>, Option<String>), CliError> {
        let resp = self
            .client
            .post(format!("{}/console/eval", self.base_url))
            .json(&serde_json::json!({ "code": code }))
            .send()
            .map_err(Self::check_error)?;
        let v: serde_json::Value = resp.json().map_err(|e| CliError::Parse(e.to_string()))?;
        let result = v["result"].as_str().map(|s| s.to_string());
        let error = v["error"].as_str().map(|s| s.to_string());
        Ok((result, error))
    }

    /// GET /style?selector=<sel> — returns computed CSS styles as pretty-printed JSON.
    fn get_style(&self, selector: Option<&str>) -> Result<String, CliError> {
        let url = if let Some(sel) = selector {
            format!("{}/style?selector={}", self.base_url, sel)
        } else {
            format!("{}/style", self.base_url)
        };
        let resp = self.client.get(&url).send().map_err(Self::check_error)?;
        let v: serde_json::Value = resp.json().map_err(|e| CliError::Parse(e.to_string()))?;
        serde_json::to_string_pretty(&v).map_err(|e| CliError::Parse(e.to_string()))
    }

    /// GET /layout — returns the plain-text layout tree.
    fn get_layout(&self) -> Result<String, CliError> {
        let resp = self
            .client
            .get(format!("{}/layout", self.base_url))
            .send()
            .map_err(Self::check_error)?;
        resp.text().map_err(|e| CliError::Http(e.to_string()))
    }

    fn get_console(&self) -> Result<Vec<ApiConsoleEntry>, CliError> {
        let resp = self
            .client
            .get(format!("{}/console", self.base_url))
            .send()
            .map_err(Self::check_error)?;
        resp.json::<Vec<ApiConsoleEntry>>()
            .map_err(|e| CliError::Parse(e.to_string()))
    }

    fn tick(&self, count: u32) -> Result<ApiTickResponse, CliError> {
        let resp = self
            .client
            .post(format!("{}/tick", self.base_url))
            .json(&serde_json::json!({ "count": count, "width": 800.0 }))
            .send()
            .map_err(Self::check_error)?;
        if !resp.status().is_success() {
            return Err(CliError::Http(format!("tick returned {}", resp.status())));
        }
        resp.json::<ApiTickResponse>()
            .map_err(|e| CliError::Parse(e.to_string()))
    }
}

// ── CliHistory ────────────────────────────────────────────────────────────────

struct CliHistory {
    entries: Vec<String>,
    /// Index of the currently-active URL in `entries`. Only valid when entries is non-empty.
    index: usize,
}

impl CliHistory {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: 0,
        }
    }

    /// Push a new URL onto the history stack. Discards any forward history.
    fn push(&mut self, url: String) {
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(url);
        self.index = self.entries.len() - 1;
    }

    /// Move back one step. Returns the URL to navigate to, or `None` if at the start.
    fn go_back(&mut self) -> Option<String> {
        if self.entries.is_empty() || self.index == 0 {
            return None;
        }
        self.index -= 1;
        Some(self.entries[self.index].clone())
    }

    /// Move forward one step. Returns the URL to navigate to, or `None` if at the end.
    fn go_forward(&mut self) -> Option<String> {
        if self.entries.is_empty() || self.index + 1 >= self.entries.len() {
            return None;
        }
        self.index += 1;
        Some(self.entries[self.index].clone())
    }

    fn current(&self) -> Option<&str> {
        self.entries.get(self.index).map(|s| s.as_str())
    }
}

// ── CLI state ─────────────────────────────────────────────────────────────────

struct CliState {
    client: DaemonClient,
    history: CliHistory,
    last_page: Option<ApiPageResponse>,
}

impl CliState {
    fn navigate(&mut self, url: &str) -> Result<(), CliError> {
        let page = self.client.navigate(url)?;
        self.history.push(url.to_string());
        render_page(&page);
        self.last_page = Some(page);
        Ok(())
    }

    fn back(&mut self) -> Result<(), CliError> {
        match self.history.go_back() {
            None => {
                eprintln!("Already at the beginning of history.");
                Ok(())
            }
            Some(url) => {
                let page = self.client.navigate(&url)?;
                render_page(&page);
                self.last_page = Some(page);
                Ok(())
            }
        }
    }

    fn forward(&mut self) -> Result<(), CliError> {
        match self.history.go_forward() {
            None => {
                eprintln!("Already at the end of history.");
                Ok(())
            }
            Some(url) => {
                let page = self.client.navigate(&url)?;
                render_page(&page);
                self.last_page = Some(page);
                Ok(())
            }
        }
    }

    fn click_by_index(&mut self, n: usize) -> Result<(), CliError> {
        let page = match &self.last_page {
            Some(p) => p.clone(),
            None => self.client.get_page()?,
        };
        let elem = page.elements.get(n - 1).ok_or_else(|| {
            CliError::NotFound(format!(
                "No element #{} on page (page has {} links).",
                n,
                page.elements.len()
            ))
        })?;
        let cx = elem.rect.x + elem.rect.w / 2.0;
        let cy = elem.rect.y + elem.rect.h / 2.0;
        let results = self.client.click_xy(cx, cy)?;
        self.handle_click_results(results)
    }

    fn click_by_text(&mut self, text: &str) -> Result<(), CliError> {
        let page = match &self.last_page {
            Some(p) => p.clone(),
            None => self.client.get_page()?,
        };
        let query = text.to_lowercase();
        let elem = page
            .elements
            .iter()
            .find(|e| {
                e.text.to_lowercase().contains(&query)
                    || e.href
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
            })
            .ok_or_else(|| CliError::NotFound(format!("No element matching \"{}\".", text)))?
            .clone();
        let cx = elem.rect.x + elem.rect.w / 2.0;
        let cy = elem.rect.y + elem.rect.h / 2.0;
        let results = self.client.click_xy(cx, cy)?;
        self.handle_click_results(results)
    }

    fn handle_click_results(&mut self, results: Vec<ClickResult>) -> Result<(), CliError> {
        for result in results {
            match result {
                ClickResult::Navigate { url } => {
                    // Follow the navigation
                    let page = self.client.navigate(&url)?;
                    self.history.push(url);
                    render_page(&page);
                    self.last_page = Some(page);
                    return Ok(());
                }
                ClickResult::FocusChanged { id } => {
                    eprintln!("[focus] Element \"{}\" focused.", id);
                }
                ClickResult::ScriptExecuted => {
                    eprintln!("[js] onclick handler executed.");
                }
                ClickResult::Nothing => {}
            }
        }
        Ok(())
    }

    fn type_into(&mut self, field: &str, value: &str) -> Result<(), CliError> {
        self.client.type_text(value)?;
        eprintln!("[type] \"{}\" → field \"{}\"", value, field);
        Ok(())
    }

    fn screenshot(&self, output: Option<&str>) -> Result<(), CliError> {
        let bytes = self.client.screenshot_bytes()?;
        let filename = output.unwrap_or("screenshot.png");
        std::fs::write(filename, &bytes).map_err(|e| CliError::Io(e.to_string()))?;
        println!("Screenshot saved to {}", filename);
        Ok(())
    }
}

// ── Markdown renderer ─────────────────────────────────────────────────────────

fn render_page(page: &ApiPageResponse) {
    println!();
    if page.title.is_empty() {
        println!("# (untitled)");
    } else {
        println!("# {}", page.title);
    }
    println!();

    if !page.markdown.is_empty() {
        println!("{}", page.markdown.trim());
        println!();
    }

    // Links section
    let links: Vec<&ApiElement> = page.elements.iter().collect();
    if links.is_empty() {
        println!("Links: (none)");
    } else {
        println!("Links:");
        for (i, elem) in links.iter().enumerate() {
            let display = if elem.text.is_empty() {
                elem.href.as_deref().unwrap_or("(no href)").to_string()
            } else {
                elem.text.clone()
            };
            let href = elem.href.as_deref().unwrap_or("");
            println!("  [{}] {}  →  {}", i + 1, display, href);
        }
    }
    println!();

    // Forms section
    if page.forms.is_empty() {
        println!("Forms: (none)");
    } else {
        println!("Forms:");
        for ctrl in &page.forms {
            let label = if ctrl.name.is_empty() {
                "(unnamed)".to_string()
            } else {
                ctrl.name.clone()
            };
            println!("  [{}] {}", ctrl.element_type, label);
        }
    }
    println!();
}

// ── Command parsing ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum ClickTarget {
    Index(usize),
    Text(String),
}

#[derive(Debug, PartialEq)]
enum Command {
    Navigate(String),
    Click(ClickTarget),
    Type {
        field: String,
        value: String,
    },
    Select {
        field: String,
        option: String,
    },
    Submit,
    Back,
    Forward,
    Screenshot(Option<String>),
    Js(String),
    /// Evaluate JS via the DevTools console REPL — result/error is echoed into the console buffer.
    Console(String),
    Style {
        selector: Option<String>,
    },
    Layout,
    Logs,
    Tick(u32),
    Help,
    Quit,
    Unknown(String),
}

fn parse_command(line: &str) -> Command {
    let line = line.trim();
    if line.is_empty() {
        return Command::Unknown(String::new());
    }

    // Split into verb + rest
    let (verb, rest) = match line.split_once(|c: char| c.is_whitespace()) {
        Some((v, r)) => (v, r.trim()),
        None => (line, ""),
    };

    match verb.to_lowercase().as_str() {
        "navigate" | "nav" | "open" | "goto" => {
            if rest.is_empty() {
                Command::Unknown("navigate requires a URL".to_string())
            } else {
                Command::Navigate(rest.to_string())
            }
        }
        "click" => {
            if rest.is_empty() {
                return Command::Unknown("click requires an argument".to_string());
            }
            // Try parsing as integer first
            if let Ok(n) = rest.trim().parse::<usize>() {
                Command::Click(ClickTarget::Index(n))
            } else {
                // Strip surrounding quotes if present
                let text = if (rest.starts_with('"') && rest.ends_with('"'))
                    || (rest.starts_with('\'') && rest.ends_with('\''))
                {
                    rest[1..rest.len() - 1].to_string()
                } else {
                    rest.to_string()
                };
                Command::Click(ClickTarget::Text(text))
            }
        }
        "type" => {
            // type <field> <value...>
            let mut parts = rest.splitn(2, |c: char| c.is_whitespace());
            let field = parts.next().unwrap_or("").to_string();
            let value = parts.next().unwrap_or("").trim().to_string();
            if field.is_empty() {
                Command::Unknown("type requires <field> <value>".to_string())
            } else {
                Command::Type { field, value }
            }
        }
        "select" => {
            let mut parts = rest.splitn(2, |c: char| c.is_whitespace());
            let field = parts.next().unwrap_or("").to_string();
            let option = parts.next().unwrap_or("").trim().to_string();
            if field.is_empty() {
                Command::Unknown("select requires <field> <option>".to_string())
            } else {
                Command::Select { field, option }
            }
        }
        "submit" => Command::Submit,
        "back" | "b" => Command::Back,
        "forward" | "fwd" | "f" => Command::Forward,
        "screenshot" | "ss" => {
            if rest.is_empty() {
                Command::Screenshot(None)
            } else {
                Command::Screenshot(Some(rest.to_string()))
            }
        }
        "js" => {
            if rest.is_empty() {
                Command::Unknown("js requires a script".to_string())
            } else {
                Command::Js(rest.to_string())
            }
        }
        "console" | "repl" => {
            if rest.is_empty() {
                Command::Unknown("console requires a JS expression".to_string())
            } else {
                Command::Console(rest.to_string())
            }
        }
        "style" => Command::Style {
            selector: if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            },
        },
        "layout" => Command::Layout,
        "logs" | "console-log" | "console-logs" => Command::Logs,
        "tick" => {
            let count = if rest.is_empty() {
                1
            } else {
                rest.parse::<u32>().unwrap_or(1)
            };
            Command::Tick(count.clamp(1, 120))
        }
        "help" | "?" | "h" => Command::Help,
        "quit" | "exit" | "q" => Command::Quit,
        other => Command::Unknown(other.to_string()),
    }
}

fn print_help() {
    println!();
    println!("browser-cli commands:");
    println!("  navigate <url>          Load a URL");
    println!("  click <N>               Click link number N");
    println!("  click \"<text>\"          Click link or button matching text");
    println!("  type <field> <value>    Type text into the focused input");
    println!("  select <field> <opt>    Choose an option in a select field");
    println!("  submit                  Submit the current form");
    println!("  back                    Go back in history");
    println!("  forward                 Go forward in history");
    println!("  screenshot [<file>]     Save a PNG screenshot (default: screenshot.png)");
    println!("  js <script>             Evaluate JavaScript and print the result");
    println!("  console <expr>          Evaluate JS via DevTools console REPL (echoes in console buffer)");
    println!(
        "  style [<selector>]      Print computed CSS styles (optionally filtered by selector)"
    );
    println!("  layout                  Print the current layout tree");
    println!("  logs                    Print browser console entries");
    println!("  tick [count]            Advance daemon JS tasks and re-render if needed");
    println!("  help                    Show this help");
    println!("  quit                    Exit the REPL");
    println!();
}

fn render_console_entries(entries: &[ApiConsoleEntry]) {
    if entries.is_empty() {
        println!("Console: (empty)");
        return;
    }

    println!("Console:");
    for entry in entries {
        println!("[{}] {} {}", entry.level, entry.timestamp, entry.message);
    }
}

// ── REPL ──────────────────────────────────────────────────────────────────────

fn dispatch_command(cmd: Command, state: &mut CliState) -> bool {
    match cmd {
        Command::Navigate(url) => {
            if let Err(e) = state.navigate(&url) {
                eprintln!("Error: {}", e);
            }
        }
        Command::Click(ClickTarget::Index(n)) => {
            if let Err(e) = state.click_by_index(n) {
                eprintln!("Error: {}", e);
            }
        }
        Command::Click(ClickTarget::Text(text)) => {
            if let Err(e) = state.click_by_text(&text) {
                eprintln!("Error: {}", e);
            }
        }
        Command::Type { field, value } => {
            if let Err(e) = state.type_into(&field, &value) {
                eprintln!("Error: {}", e);
            }
        }
        Command::Select { field, option } => {
            // Select is proxied through JS evaluation
            let script = format!(
                "(function(){{ var s=document.querySelector('[name=\"{}\"]'); if(s) s.value='{}'; }})();",
                field.replace('"', "\\\""),
                option.replace('"', "\\\"")
            );
            match state.client.evaluate_js(&script) {
                Ok(_) => eprintln!("[select] {} → {}", field, option),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
        Command::Submit => {
            if let Err(e) = state.client.submit() {
                eprintln!("Error: {}", e);
            } else {
                eprintln!("[submit] Form submitted.");
            }
        }
        Command::Back => {
            if let Err(e) = state.back() {
                eprintln!("Error: {}", e);
            }
        }
        Command::Forward => {
            if let Err(e) = state.forward() {
                eprintln!("Error: {}", e);
            }
        }
        Command::Screenshot(path) => {
            if let Err(e) = state.screenshot(path.as_deref()) {
                eprintln!("Error: {}", e);
            }
        }
        Command::Js(script) => match state.client.evaluate_js(&script) {
            Ok(result) => println!("js result: {}", result),
            Err(e) => eprintln!("Error: {}", e),
        },
        Command::Console(code) => match state.client.console_eval(&code) {
            Ok((Some(result), _)) => println!("< {}", result),
            Ok((_, Some(error))) => eprintln!("< [error] {}", error),
            Ok((None, None)) => {}
            Err(e) => eprintln!("Error: {}", e),
        },
        Command::Style { selector } => match state.client.get_style(selector.as_deref()) {
            Ok(text) => println!("{}", text),
            Err(e) => eprintln!("Error: {}", e),
        },
        Command::Layout => match state.client.get_layout() {
            Ok(text) => println!("{}", text),
            Err(e) => eprintln!("Error: {}", e),
        },
        Command::Logs => match state.client.get_console() {
            Ok(entries) => render_console_entries(&entries),
            Err(e) => eprintln!("Error: {}", e),
        },
        Command::Tick(count) => match state.client.tick(count) {
            Ok(resp) => println!(
                "tick: ticks={} worked={} rerendered={}",
                resp.ticks, resp.worked, resp.rerendered
            ),
            Err(e) => eprintln!("Error: {}", e),
        },
        Command::Help => print_help(),
        Command::Quit => {
            println!("Goodbye.");
            return true; // signal quit
        }
        Command::Unknown(s) if s.is_empty() => {} // blank line — do nothing
        Command::Unknown(s) => eprintln!("Unknown command: \"{}\". Type 'help' for commands.", s),
    }
    false
}

fn run_repl(state: &mut CliState) {
    let mut editor = match DefaultEditor::new() {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Failed to initialize readline: {}", err);
            return;
        }
    };

    loop {
        let prompt = if let Some(url) = state.history.current() {
            format!("\n[browser: {}] > ", shorten_url(url))
        } else {
            "\n[browser] > ".to_string()
        };

        match editor.readline(&prompt) {
            Ok(line) => {
                let _ = editor.add_history_entry(line.as_str());
                let cmd = parse_command(&line);
                if dispatch_command(cmd, state) {
                    break; // quit was requested
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C — continue loop
                eprintln!("(Ctrl+C — type 'quit' to exit)");
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D
                println!("\nGoodbye.");
                break;
            }
            Err(err) => {
                eprintln!("Readline error: {}", err);
                break;
            }
        }
    }
}

fn shorten_url(url: &str) -> String {
    // Show only host + path, truncated to 40 chars for the prompt
    let display = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    if display.len() > 40 {
        format!("{}...", &display[..37])
    } else {
        display.to_string()
    }
}

fn run_single_command(args: &[String], state: &mut CliState) {
    let line = args.join(" ");
    let cmd = parse_command(&line);
    dispatch_command(cmd, state);
}

// ── Port resolution ───────────────────────────────────────────────────────────

fn resolve_port(args: &[String]) -> u16 {
    // Check env var first
    if let Ok(val) = std::env::var("BROWSER_DAEMON_PORT") {
        if let Ok(p) = val.parse::<u16>() {
            return p;
        }
    }
    // Check --port flag
    if let Some(pos) = args.iter().position(|a| a == "--port") {
        if let Some(val) = args.get(pos + 1) {
            if let Ok(p) = val.parse::<u16>() {
                return p;
            }
        }
    }
    7070
}

/// Return args with `--port <N>` stripped out.
fn strip_port_args(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--port" {
            skip_next = true;
            continue;
        }
        result.push(arg.clone());
    }
    result
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let port = resolve_port(&raw_args);
    let remaining = strip_port_args(&raw_args);

    let mut state = CliState {
        client: DaemonClient::new(port),
        history: CliHistory::new(),
        last_page: None,
    };

    if remaining.is_empty() {
        println!("browser-cli — connecting to daemon on port {}", port);
        println!("Type 'help' for available commands, 'quit' to exit.");
        run_repl(&mut state);
    } else {
        run_single_command(&remaining, &mut state);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- DaemonClient port tests --

    #[test]
    fn test_daemon_client_default_port() {
        let client = DaemonClient::new(7070);
        assert_eq!(client.base_url, "http://localhost:7070");
    }

    #[test]
    fn test_daemon_client_custom_port() {
        let client = DaemonClient::new(8080);
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    // -- CliHistory tests --

    #[test]
    fn test_cli_history_no_back_at_start() {
        let mut h = CliHistory::new();
        assert!(h.go_back().is_none());
    }

    #[test]
    fn test_cli_history_no_forward_at_end() {
        let mut h = CliHistory::new();
        h.push("https://a.com".to_string());
        assert!(h.go_forward().is_none());
    }

    #[test]
    fn test_cli_history_push_and_back() {
        let mut h = CliHistory::new();
        h.push("https://a.com".to_string());
        h.push("https://b.com".to_string());
        let url = h.go_back().expect("should have back");
        assert_eq!(url, "https://a.com");
        assert_eq!(h.index, 0);
    }

    #[test]
    fn test_cli_history_forward_after_back() {
        let mut h = CliHistory::new();
        h.push("https://a.com".to_string());
        h.push("https://b.com".to_string());
        h.go_back();
        let url = h.go_forward().expect("should have forward");
        assert_eq!(url, "https://b.com");
        assert_eq!(h.index, 1);
    }

    #[test]
    fn test_cli_history_push_clears_forward() {
        let mut h = CliHistory::new();
        h.push("https://a.com".to_string());
        h.push("https://b.com".to_string());
        h.go_back();
        h.push("https://c.com".to_string()); // should discard b
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[1], "https://c.com");
        assert!(h.go_forward().is_none());
    }

    #[test]
    fn test_cli_history_current_none_when_empty() {
        let h = CliHistory::new();
        assert!(h.current().is_none());
    }

    #[test]
    fn test_cli_history_current_after_push() {
        let mut h = CliHistory::new();
        h.push("https://example.com".to_string());
        assert_eq!(h.current(), Some("https://example.com"));
    }

    // -- Command parsing tests --

    #[test]
    fn test_parse_command_navigate() {
        let cmd = parse_command("navigate https://example.com");
        assert_eq!(cmd, Command::Navigate("https://example.com".to_string()));
    }

    #[test]
    fn test_parse_command_navigate_alias() {
        let cmd = parse_command("open https://rust-lang.org");
        assert_eq!(cmd, Command::Navigate("https://rust-lang.org".to_string()));
    }

    #[test]
    fn test_parse_command_click_index() {
        let cmd = parse_command("click 3");
        assert_eq!(cmd, Command::Click(ClickTarget::Index(3)));
    }

    #[test]
    fn test_parse_command_click_quoted_text() {
        let cmd = parse_command("click \"Login\"");
        assert_eq!(cmd, Command::Click(ClickTarget::Text("Login".to_string())));
    }

    #[test]
    fn test_parse_command_click_bare_text() {
        let cmd = parse_command("click More information");
        assert_eq!(
            cmd,
            Command::Click(ClickTarget::Text("More information".to_string()))
        );
    }

    #[test]
    fn test_parse_command_type() {
        let cmd = parse_command("type username admin");
        assert_eq!(
            cmd,
            Command::Type {
                field: "username".to_string(),
                value: "admin".to_string()
            }
        );
    }

    #[test]
    fn test_parse_command_select() {
        let cmd = parse_command("select theme dark");
        assert_eq!(
            cmd,
            Command::Select {
                field: "theme".to_string(),
                option: "dark".to_string()
            }
        );
    }

    #[test]
    fn test_parse_command_submit() {
        assert_eq!(parse_command("submit"), Command::Submit);
    }

    #[test]
    fn test_parse_command_back() {
        assert_eq!(parse_command("back"), Command::Back);
        assert_eq!(parse_command("b"), Command::Back);
    }

    #[test]
    fn test_parse_command_forward() {
        assert_eq!(parse_command("forward"), Command::Forward);
        assert_eq!(parse_command("fwd"), Command::Forward);
        assert_eq!(parse_command("f"), Command::Forward);
    }

    #[test]
    fn test_parse_command_screenshot_no_file() {
        assert_eq!(parse_command("screenshot"), Command::Screenshot(None));
    }

    #[test]
    fn test_parse_command_screenshot_with_file() {
        let cmd = parse_command("screenshot out.png");
        assert_eq!(cmd, Command::Screenshot(Some("out.png".to_string())));
    }

    #[test]
    fn test_parse_command_js() {
        let cmd = parse_command("js document.title");
        assert_eq!(cmd, Command::Js("document.title".to_string()));
    }

    #[test]
    fn test_parse_command_js_no_script() {
        let cmd = parse_command("js");
        assert!(matches!(cmd, Command::Unknown(_)));
    }

    #[test]
    fn test_parse_command_console() {
        let cmd = parse_command("console 1 + 1");
        assert_eq!(cmd, Command::Console("1 + 1".to_string()));
    }

    #[test]
    fn test_parse_command_console_alias() {
        let cmd = parse_command("repl document.title");
        assert_eq!(cmd, Command::Console("document.title".to_string()));
    }

    #[test]
    fn test_parse_command_console_no_expr() {
        let cmd = parse_command("console");
        assert!(matches!(cmd, Command::Unknown(_)));
    }

    #[test]
    fn test_parse_command_style_no_selector() {
        assert_eq!(parse_command("style"), Command::Style { selector: None });
    }

    #[test]
    fn test_parse_command_style_with_selector() {
        assert_eq!(
            parse_command("style body"),
            Command::Style {
                selector: Some("body".to_string())
            }
        );
    }

    #[test]
    fn test_parse_command_layout() {
        assert_eq!(parse_command("layout"), Command::Layout);
    }

    #[test]
    fn test_parse_command_logs() {
        assert_eq!(parse_command("logs"), Command::Logs);
        assert_eq!(parse_command("console-logs"), Command::Logs);
    }

    #[test]
    fn test_parse_command_tick() {
        assert_eq!(parse_command("tick"), Command::Tick(1));
        assert_eq!(parse_command("tick 3"), Command::Tick(3));
        assert_eq!(parse_command("tick nope"), Command::Tick(1));
        assert_eq!(parse_command("tick 500"), Command::Tick(120));
    }

    #[test]
    fn test_parse_command_quit_variants() {
        assert_eq!(parse_command("quit"), Command::Quit);
        assert_eq!(parse_command("exit"), Command::Quit);
        assert_eq!(parse_command("q"), Command::Quit);
    }

    #[test]
    fn test_parse_command_help() {
        assert_eq!(parse_command("help"), Command::Help);
        assert_eq!(parse_command("?"), Command::Help);
    }

    #[test]
    fn test_parse_command_unknown() {
        let cmd = parse_command("xyzzy");
        assert_eq!(cmd, Command::Unknown("xyzzy".to_string()));
    }

    #[test]
    fn test_parse_command_empty() {
        let cmd = parse_command("   ");
        assert_eq!(cmd, Command::Unknown(String::new()));
    }

    // -- Render page (no-panic tests) --

    #[test]
    fn test_render_page_no_panic_empty() {
        let page = ApiPageResponse {
            url: "https://example.com".to_string(),
            title: String::new(),
            markdown: String::new(),
            elements: vec![],
            forms: vec![],
            width: 800,
            height: 600,
        };
        // Should not panic
        render_page(&page);
    }

    #[test]
    fn test_render_page_with_links_no_panic() {
        let page = ApiPageResponse {
            url: "https://example.com".to_string(),
            title: "Example Domain".to_string(),
            markdown: "This is a test page.".to_string(),
            elements: vec![
                ApiElement {
                    id: "e0".to_string(),
                    element_type: "link".to_string(),
                    text: "More information".to_string(),
                    href: Some("https://www.iana.org/domains/reserved".to_string()),
                    rect: ApiRect {
                        x: 100.0,
                        y: 200.0,
                        w: 120.0,
                        h: 20.0,
                    },
                },
                ApiElement {
                    id: "e1".to_string(),
                    element_type: "link".to_string(),
                    text: String::new(),
                    href: Some("https://another.com".to_string()),
                    rect: ApiRect {
                        x: 0.0,
                        y: 0.0,
                        w: 50.0,
                        h: 20.0,
                    },
                },
            ],
            forms: vec![],
            width: 800,
            height: 600,
        };
        // Should not panic
        render_page(&page);
    }

    // -- Port resolution tests --

    #[test]
    fn test_resolve_port_default() {
        let args: Vec<String> = vec![];
        // Without env var, expect 7070 (only testable if BROWSER_DAEMON_PORT is unset)
        if std::env::var("BROWSER_DAEMON_PORT").is_err() {
            assert_eq!(resolve_port(&args), 7070);
        }
    }

    #[test]
    fn test_resolve_port_flag() {
        let args: Vec<String> = vec!["--port".to_string(), "9090".to_string()];
        if std::env::var("BROWSER_DAEMON_PORT").is_err() {
            assert_eq!(resolve_port(&args), 9090);
        }
    }

    #[test]
    fn test_strip_port_args() {
        let args: Vec<String> = vec![
            "--port".to_string(),
            "7070".to_string(),
            "navigate".to_string(),
            "https://example.com".to_string(),
        ];
        let stripped = strip_port_args(&args);
        assert_eq!(stripped, vec!["navigate", "https://example.com"]);
    }

    #[test]
    fn test_shorten_url_strips_https() {
        let s = shorten_url("https://example.com/path");
        assert_eq!(s, "example.com/path");
    }

    #[test]
    fn test_shorten_url_truncates_long() {
        let long = "https://".to_string() + &"a".repeat(50);
        let s = shorten_url(&long);
        assert!(s.ends_with("..."));
        assert!(s.len() <= 43); // 40 chars + "..."
    }

    // -- Error reporting tests --

    #[test]
    fn test_parse_error_includes_body_hint() {
        // Simulate what happens when the daemon returns non-JSON (e.g., a panic message).
        let invalid_json = "thread 'tokio-runtime' panicked at 'byte index 5...'";
        let err = serde_json::from_str::<ApiPageResponse>(invalid_json)
            .map_err(|e| {
                let preview = if invalid_json.len() > 200 {
                    &invalid_json[..200]
                } else {
                    invalid_json
                };
                CliError::Parse(format!("{} (raw body: {})", e, preview))
            })
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Parse error:"),
            "expected 'Parse error:' in: {}",
            msg
        );
        assert!(
            msg.contains("raw body:"),
            "expected 'raw body:' in: {}",
            msg
        );
        assert!(msg.contains("panicked"), "expected panic text in: {}", msg);
    }
}
