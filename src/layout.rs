use crate::style::StyledNode;
use crate::css::{Value, Unit};

#[derive(Default, Debug, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct LayoutBox<'a> {
    pub dimensions: Rect,
    pub style_node: &'a StyledNode,
    pub children: Vec<LayoutBox<'a>>,
    pub link_url: Option<String>,
}

pub fn build_layout_tree<'a>(
    style_node: &'a StyledNode,
    current_x: f32,
    mut current_y: f32,
    parent_width: f32
) -> (Option<LayoutBox<'a>>, f32) {
    // 1. display: none 처리
    if let Some(Value::Keyword(d)) = style_node.specified_values.get("display") {
        if d == "none" {
            return (None, current_y);
        }
    }

    // 2. 여백 및 간격 추출
    let padding = match style_node.specified_values.get("padding") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };
    let margin = match style_node.specified_values.get("margin") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };

    // 3. 링크 추출
    let mut link_url = None;
    if let markup5ever_rcdom::NodeData::Element { ref attrs, ref name, .. } = style_node.node.data {
        if name.local.to_string() == "a" {
            for attr in attrs.borrow().iter() {
                if attr.name.local.to_string() == "href" {
                    link_url = Some(attr.value.to_string());
                }
            }
        }
    }

    // 4. 절대 좌표 및 크기 계산
    let mut layout_width = parent_width - (margin * 2.0);
    if let Some(Value::Length(w, Unit::Px)) = style_node.specified_values.get("width") {
        layout_width = *w;
    } else if let Some(Value::Length(w, Unit::Vw)) = style_node.specified_values.get("width") {
        layout_width = (parent_width * (*w / 100.0)).min(parent_width - (margin * 2.0));
    }

    let box_x = current_x + margin;
    let box_y = current_y + margin;
    
    let mut layout = LayoutBox {
        dimensions: Rect {
            x: box_x,
            y: box_y,
            width: layout_width,
            height: 0.0, // 자식 노드 순회 후 결정
        },
        style_node,
        children: Vec::new(),
        link_url,
    };

    let child_start_x = box_x + padding;
    let mut child_current_y = box_y + padding;

    // 5. 자식 노드 레이아웃 (재귀)
    for child in &style_node.children {
        // 레이아웃에서 제외할 태그들
        if let markup5ever_rcdom::NodeData::Element { ref name, .. } = child.node.data {
            let t = name.local.to_string();
            if t == "head" || t == "style" || t == "meta" || t == "title" || t == "script" {
                continue;
            }
        }

        let (child_layout, next_y) = build_layout_tree(
            child,
            child_start_x,
            child_current_y,
            layout_width - (padding * 2.0)
        );

        if let Some(child_box) = child_layout {
            layout.children.push(child_box);
            child_current_y = next_y;
        }
    }

    // 6. 최종 높이 결정
    let final_y = child_current_y + padding + margin;
    layout.dimensions.height = final_y - box_y - margin;

    (Some(layout), final_y)
}

impl<'a> LayoutBox<'a> {
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&'a StyledNode> {
        if x >= self.dimensions.x && x <= self.dimensions.x + self.dimensions.width &&
           y >= self.dimensions.y && y <= self.dimensions.y + self.dimensions.height {
            
            for child in self.children.iter().rev() {
                if let Some(node) = child.hit_test(x, y) {
                    return Some(node);
                }
            }
            return Some(self.style_node);
        }
        None
    }

    pub fn get_links(&self) -> Vec<(Rect, String)> {
        let mut links = Vec::new();
        if let Some(ref url) = self.link_url {
            links.push((self.dimensions, url.clone()));
        }
        for child in &self.children {
            links.extend(child.get_links());
        }
        links
    }
}
