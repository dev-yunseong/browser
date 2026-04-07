use eframe::egui;
use poll_promise::Promise;
use std::error::Error;
use url::Url;
use std::collections::HashMap;

mod dom;
mod css;
mod style;
mod layout;
mod render;
mod js;

struct BrowserApp {
    url: String,
    history: Vec<String>,
    history_index: usize,
    content_promise: Option<Promise<Result<PageData, String>>>,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    current_links: Vec<(layout::Rect, String)>,
    current_form_controls: Vec<(layout::Rect, String)>, 
    form_values: HashMap<usize, String>, 
}

struct PageData {
    pixmap_bytes: Vec<u8>,
    width: u32,
    height: u32,
    links: Vec<(layout::Rect, String)>,
    form_controls: Vec<(layout::Rect, String)>,
}

impl BrowserApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // [FONT CONFIG] Setup egui to support Korean
        let mut fonts = egui::FontDefinitions::default();
        let nanum_data = include_bytes!("../assets/fonts/NanumGothic.ttf");
        fonts.font_data.insert("nanum".to_owned(), egui::FontData::from_static(nanum_data));
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "nanum".to_owned());
        fonts.families.get_mut(&egui::FontFamily::Monospace).unwrap().push("nanum".to_owned());
        cc.egui_ctx.set_fonts(fonts);

        Self {
            url: "https://yunseong.dev".to_string(),
            history: vec![],
            history_index: 0,
            content_promise: None,
            texture: None,
            error: None,
            current_links: vec![],
            current_form_controls: vec![],
            form_values: HashMap::new(),
        }
    }

    fn load_url(&mut self, url: String) {
        if self.history.is_empty() || self.history[self.history_index] != url {
            self.history.truncate(self.history_index + 1);
            self.history.push(url.clone());
            self.history_index = self.history.len() - 1;
        }
        self.load_url_direct(url);
    }

    fn navigate_back(&mut self) {
        if self.history_index > 0 {
            self.history_index -= 1;
            let url = self.history[self.history_index].clone();
            self.load_url_direct(url);
        }
    }

    fn navigate_forward(&mut self) {
        if self.history_index + 1 < self.history.len() {
            self.history_index += 1;
            let url = self.history[self.history_index].clone();
            self.load_url_direct(url);
        }
    }

    fn load_url_direct(&mut self, url: String) {
        self.url = url.clone();
        self.error = None;
        self.content_promise = Some(Promise::spawn_thread("fetcher", move || {
            fetch_and_process(&url).map_err(|e| e.to_string())
        }));
    }
}

fn fetch_and_process(url_str: &str) -> Result<PageData, Box<dyn Error + Send + Sync>> {
    let response = reqwest::blocking::get(url_str)?;
    let body = response.text()?;
    let base_url = Url::parse(url_str)?;

    let dom_tree = dom::parse_html(&body);
    let mut css_source = style::extract_css_from_dom(&dom_tree.document);

    let external_links = style::extract_external_css_links(&dom_tree.document);
    for link in external_links {
        let abs_url = base_url.join(&link).map(|u| u.to_string()).unwrap_or(link.clone());
        if let Ok(resp) = reqwest::blocking::get(&abs_url) {
            if let Ok(external_css) = resp.text() {
                css_source.push_str(&external_css);
            }
        }
    }

    let stylesheet = css::parse_css(&css_source);
    let style_tree = style::build_style_tree(&dom_tree.document, &stylesheet, None);
    
    let mut js_runtime = js::JsRuntime::new();
    let scripts = js::extract_scripts_from_dom(&dom_tree.document);
    for script in scripts {
        js_runtime.execute(&script);
    }

    let width = 800;
    let (layout_tree, _, final_y) = layout::build_layout_tree(&style_tree, 0.0, 0.0, 0.0, width as f32);

    let height = (final_y.ceil() as u32).clamp(600, 16384);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);
    
    let mut links = Vec::new();
    let mut form_controls = Vec::new();

    if let Some(ref layout) = layout_tree {
        render::render_layout_tree(layout, &mut pixmap);
        links = layout.get_links();
        for (rect, node) in layout.get_form_controls() {
            let mut initial_val = String::new();
            if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = node.node.data {
                for attr in attrs.borrow().iter() {
                    if attr.name.local.to_string() == "value" {
                        initial_val = attr.value.to_string();
                    }
                }
            }
            form_controls.push((rect, initial_val));
        }
    }

    let absolute_links = links.into_iter().map(|(rect, link)| {
        let abs_link = base_url.join(&link).map(|u| u.to_string()).unwrap_or(link);
        (rect, abs_link)
    }).collect();

    Ok(PageData {
        pixmap_bytes: pixmap.data().to_vec(),
        width,
        height,
        links: absolute_links,
        form_controls,
    })
}

impl eframe::App for BrowserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("browser_chrome").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("<-").clicked() { self.navigate_back(); }
                if ui.button("->").clicked() { self.navigate_forward(); }
                if ui.button("R").clicked() { let url = self.url.clone(); self.load_url_direct(url); }

                ui.label("URL:");
                let edit = ui.text_edit_singleline(&mut self.url);
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let url = self.url.clone();
                    self.load_url(url);
                }
                if ui.button("Go").clicked() {
                    let url = self.url.clone();
                    self.load_url(url);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(promise) = &self.content_promise {
                match promise.ready() {
                    None => { ui.spinner(); ui.label("Loading..."); }
                    Some(Err(e)) => { self.error = Some(e.clone()); self.content_promise = None; }
                    Some(Ok(page_data)) => {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [page_data.width as usize, page_data.height as usize],
                            &page_data.pixmap_bytes,
                        );
                        self.texture = Some(ctx.load_texture("page_content", image, Default::default()));
                        self.current_links = page_data.links.clone();
                        self.current_form_controls = page_data.form_controls.clone();
                        self.form_values.clear();
                        for (i, (_, val)) in page_data.form_controls.iter().enumerate() {
                            self.form_values.insert(i, val.clone());
                        }
                        self.content_promise = None;
                    }
                }
            }

            if let Some(err) = &self.error { ui.colored_label(egui::Color32::RED, format!("Error: {}", err)); }

            if let Some(texture) = &self.texture {
                let mut url_to_load = None;
                egui::ScrollArea::both().show(ui, |ui| {
                    let (rect, response) = ui.allocate_at_least(texture.size_vec2(), egui::Sense::click());
                    ui.painter().image(texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                    
                    // Overlay Form Controls (Before link click check to avoid blocking)
                    for (i, (l_rect, _)) in self.current_form_controls.iter().enumerate() {
                        let val = self.form_values.entry(i).or_default();
                        let screen_rect = egui::Rect::from_min_size(
                            rect.min + egui::vec2(l_rect.x, l_rect.y),
                            egui::vec2(l_rect.width, l_rect.height)
                        );
                        
                        // Use a unique ID for each input
                        ui.put(screen_rect, egui::TextEdit::singleline(val).id_source(i));
                    }

                    if response.clicked() {
                        if let Some(pointer_pos) = response.interact_pointer_pos() {
                            let rel_pos = pointer_pos - rect.min;
                            for (l_rect, link) in &self.current_links {
                                if rel_pos.x >= l_rect.x && rel_pos.x <= l_rect.x + l_rect.width &&
                                   rel_pos.y >= l_rect.y && rel_pos.y <= l_rect.y + l_rect.height {
                                    url_to_load = Some(link.clone());
                                    break;
                                }
                            }
                        }
                    }

                    if let Some(pointer_pos) = response.hover_pos() {
                        let rel_pos = pointer_pos - rect.min;
                        for (l_rect, _) in &self.current_links {
                            if rel_pos.x >= l_rect.x && rel_pos.x <= l_rect.x + l_rect.width &&
                               rel_pos.y >= l_rect.y && rel_pos.y <= l_rect.y + l_rect.height {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                break;
                            }
                        }
                    }
                });

                if let Some(url) = url_to_load { self.load_url(url); }
            }
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Aura Browser",
        options,
        Box::new(|cc| Ok(Box::new(BrowserApp::new(cc)))),
    )
}
