use eframe::egui;
use poll_promise::Promise;
use std::error::Error;
use url::Url;

mod dom;
mod css;
mod style;
mod layout;
mod render;

struct BrowserApp {
    url: String,
    history: Vec<String>,
    history_index: usize,
    content_promise: Option<Promise<Result<PageData, String>>>,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    current_links: Vec<(layout::Rect, String)>,
}

struct PageData {
    pixmap_bytes: Vec<u8>,
    width: u32,
    height: u32,
    links: Vec<(layout::Rect, String)>,
}

impl BrowserApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            url: "https://yunseong.dev".to_string(),
            history: vec![],
            history_index: 0,
            content_promise: None,
            texture: None,
            error: None,
            current_links: vec![],
        }
    }

    fn load_url(&mut self, url: String) {
        // Add to history if new
        if self.history.is_empty() || self.history[self.history_index] != url {
            self.history.truncate(self.history_index + 1);
            self.history.push(url.clone());
            self.history_index = self.history.len() - 1;
        }
        
        self.url = url.clone();
        self.error = None;
        self.content_promise = Some(Promise::spawn_thread("fetcher", move || {
            fetch_and_process(&url).map_err(|e| e.to_string())
        }));
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
    let css_source = style::extract_css_from_dom(&dom_tree.document);
    let stylesheet = css::parse_css(&css_source);
    let style_tree = style::build_style_tree(&dom_tree.document, &stylesheet);
    
    let width = 800;
    let (layout_tree, _, final_y) = layout::build_layout_tree(&style_tree, 0.0, 0.0, 0.0, width as f32);

    // Set height to content height, with a minimum of 600
    let height = (final_y.ceil() as u32).max(600);
    
    let mut pixmap = tiny_skia::Pixmap::new(width, height).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);
    
    let mut links = Vec::new();
    if let Some(layout) = layout_tree {
        println!("--- Layout Tree (Content Height: {}) ---", height);
        layout::print_layout_tree(&layout, 0);
        render::render_layout_tree(&layout, &mut pixmap);
        links = layout.get_links();
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
    })
}

impl eframe::App for BrowserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("browser_chrome").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("<-").clicked() {
                    self.navigate_back();
                }
                if ui.button("->").clicked() {
                    self.navigate_forward();
                }
                if ui.button("R").clicked() {
                    let url = self.url.clone();
                    self.load_url_direct(url);
                }

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
                    None => {
                        ui.spinner();
                        ui.label("Loading...");
                    }
                    Some(Err(e)) => {
                        self.error = Some(e.clone());
                        self.content_promise = None;
                    }
                    Some(Ok(page_data)) => {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [page_data.width as usize, page_data.height as usize],
                            &page_data.pixmap_bytes,
                        );
                        self.texture = Some(ctx.load_texture("page_content", image, Default::default()));
                        self.current_links = page_data.links.clone();
                        self.content_promise = None;
                    }
                }
            }

            if let Some(err) = &self.error {
                ui.colored_label(egui::Color32::RED, format!("Error: {}", err));
            }

            if let Some(texture) = &self.texture {
                let mut url_to_load = None;
                egui::ScrollArea::both().show(ui, |ui| {
                    let response = ui.image((texture.id(), texture.size_vec2()));
                    
                    if response.clicked() {
                        if let Some(pointer_pos) = response.interact_pointer_pos() {
                            let rel_pos = pointer_pos - response.rect.min;
                            // Check link clicks
                            for (rect, link) in &self.current_links {
                                if rel_pos.x >= rect.x && rel_pos.x <= rect.x + rect.width &&
                                   rel_pos.y >= rect.y && rel_pos.y <= rect.y + rect.height {
                                    url_to_load = Some(link.clone());
                                    break;
                                }
                            }
                        }
                    }

                    // Change cursor if hovering a link
                    if let Some(pointer_pos) = response.hover_pos() {
                        let rel_pos = pointer_pos - response.rect.min;
                        for (rect, _) in &self.current_links {
                            if rel_pos.x >= rect.x && rel_pos.x <= rect.x + rect.width &&
                               rel_pos.y >= rect.y && rel_pos.y <= rect.y + rect.height {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                break;
                            }
                        }
                    }
                });

                if let Some(url) = url_to_load {
                    self.load_url(url);
                }
            } else {
                ui.label("Enter a URL and press Go to load a page.");
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
