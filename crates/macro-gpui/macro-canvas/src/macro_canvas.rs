use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use uuid::Uuid;

pub type CanvasId = String;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
        }
    }
}

impl Viewport {
    pub const MIN_SCALE: f64 = 0.1;
    pub const MAX_SCALE: f64 = 4.0;

    pub fn pan(&mut self, delta: Point) {
        self.x += delta.x;
        self.y += delta.y;
    }

    pub fn zoom_at(&mut self, requested_scale: f64, screen_point: Point) {
        let next = requested_scale.clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        let canvas_x = (screen_point.x - self.x) / self.scale;
        let canvas_y = (screen_point.y - self.y) / self.scale;
        self.x = screen_point.x - canvas_x * next;
        self.y = screen_point.y - canvas_y * next;
        self.scale = next;
    }

    pub fn screen_to_canvas(&self, point: Point) -> Point {
        Point {
            x: (point.x - self.x) / self.scale,
            y: (point.y - self.y) / self.scale,
        }
    }

    pub fn canvas_to_screen(&self, point: Point) -> Point {
        Point {
            x: self.x + point.x * self.scale,
            y: self.y + point.y * self.scale,
        }
    }

    pub fn fit_bounds(&mut self, origin: Point, size: Point, viewport: Point, padding: f64) {
        if size.x <= 0.0 || size.y <= 0.0 || viewport.x <= 0.0 || viewport.y <= 0.0 {
            *self = Self::default();
            return;
        }
        let available_width = (viewport.x - padding * 2.0).max(1.0);
        let available_height = (viewport.y - padding * 2.0).max(1.0);
        self.scale = (available_width / size.x)
            .min(available_height / size.y)
            .clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        self.x = (viewport.x - size.x * self.scale) / 2.0 - origin.x * self.scale;
        self.y = (viewport.y - size.y * self.scale) / 2.0 - origin.y * self.scale;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_end_style: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_end_style: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_style: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_color: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasNode {
    pub id: CanvasId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<CanvasId>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub edges: Vec<CanvasId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<CanvasStyle>,
    #[serde(default)]
    pub sort_order: f64,
    #[serde(default)]
    pub layer: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub kind: CanvasNodeKind,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CanvasNode {
    pub fn text(text: impl Into<String>, x: f64, y: f64) -> Self {
        Self::new(
            CanvasNodeKind::Text {
                text: text.into(),
                follow_text_width: Some(true),
            },
            x,
            y,
            200.0,
            48.0,
        )
    }

    pub fn shape(shape: ShapeType, x: f64, y: f64) -> Self {
        Self::new(CanvasNodeKind::Shape { shape }, x, y, 160.0, 100.0)
    }

    pub fn new(kind: CanvasNodeKind, x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            group_id: None,
            x,
            y,
            width,
            height,
            edges: Vec::new(),
            style: None,
            sort_order: 0.0,
            layer: 0.0,
            label: None,
            kind,
            extra: Map::new(),
        }
    }

    pub fn center(&self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CanvasNodeKind {
    Text {
        text: String,
        #[serde(
            rename = "followTextWidth",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        follow_text_width: Option<bool>,
    },
    #[serde(rename = "entitymention")]
    EntityMention {
        #[serde(rename = "entityType")]
        entity_type: CanvasMentionType,
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
        #[serde(
            rename = "mentionUuid",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        mention_uuid: Option<String>,
    },
    Link {
        url: String,
    },
    Shape {
        #[serde(default)]
        shape: ShapeType,
    },
    Pencil {
        coords: Vec<(f64, f64)>,
        #[serde(rename = "wScale", default = "one")]
        w_scale: f64,
        #[serde(rename = "hScale", default = "one")]
        h_scale: f64,
    },
    Image {
        #[serde(default)]
        status: MediaStatus,
        uuid: String,
        #[serde(rename = "flipX", default)]
        flip_x: bool,
        #[serde(rename = "flipY", default)]
        flip_y: bool,
    },
    Video {
        #[serde(default)]
        status: MediaStatus,
        uuid: String,
        #[serde(rename = "flipX", default)]
        flip_x: bool,
        #[serde(rename = "flipY", default)]
        flip_y: bool,
    },
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeType {
    #[default]
    Rectangle,
    Ellipse,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaStatus {
    #[default]
    Dss,
    Static,
    Loading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanvasMentionType {
    Document,
    Chat,
    Project,
    Channel,
    Email,
    Call,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeSide {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EdgeEnd {
    Connected { node: CanvasId, side: EdgeSide },
    Free { x: f64, y: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEdge {
    pub id: CanvasId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<CanvasId>,
    pub from: EdgeEnd,
    pub to: EdgeEnd,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<CanvasStyle>,
    #[serde(default)]
    pub sort_order: f64,
    #[serde(default)]
    pub layer: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasGroup {
    pub id: CanvasId,
    #[serde(default)]
    pub child_nodes: Vec<CanvasId>,
    #[serde(default)]
    pub child_edges: Vec<CanvasId>,
    #[serde(default)]
    pub sort_order: f64,
    #[serde(default)]
    pub layer: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<CanvasNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<CanvasEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<CanvasGroup>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Canvas {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let mut value: Value = serde_json::from_slice(bytes).context("invalid canvas JSON")?;
        migrate_legacy_nodes(&mut value);
        let canvas: Self = serde_json::from_value(value).context("invalid Macro canvas schema")?;
        canvas.validate()?;
        Ok(canvas)
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).context("failed to encode Macro canvas")
    }

    pub fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        for node in &self.nodes {
            anyhow::ensure!(ids.insert(&node.id), "duplicate canvas id {}", node.id);
            anyhow::ensure!(
                [node.x, node.y, node.width, node.height]
                    .into_iter()
                    .all(f64::is_finite),
                "node {} has non-finite geometry",
                node.id
            );
            anyhow::ensure!(
                node.width >= 0.0 && node.height >= 0.0,
                "node {} has negative size",
                node.id
            );
            validate_style(node.style.as_ref(), &node.id)?;
            match &node.kind {
                CanvasNodeKind::Link { url } => anyhow::ensure!(
                    url.starts_with("https://") || url.starts_with("http://"),
                    "link node {} has an unsafe URL",
                    node.id
                ),
                CanvasNodeKind::Pencil {
                    coords,
                    w_scale,
                    h_scale,
                } => {
                    anyhow::ensure!(
                        coords
                            .iter()
                            .flat_map(|(x, y)| [x, y])
                            .all(|value| value.is_finite()),
                        "pencil node {} has non-finite coordinates",
                        node.id
                    );
                    anyhow::ensure!(
                        w_scale.is_finite() && h_scale.is_finite(),
                        "pencil node {} has non-finite scale",
                        node.id
                    );
                }
                _ => {}
            }
        }
        let node_ids = ids.clone();
        let edge_ids = self
            .edges
            .iter()
            .map(|edge| edge.id.clone())
            .collect::<HashSet<_>>();
        for edge in &self.edges {
            anyhow::ensure!(ids.insert(&edge.id), "duplicate canvas id {}", edge.id);
            validate_style(edge.style.as_ref(), &edge.id)?;
            for end in [&edge.from, &edge.to] {
                match end {
                    EdgeEnd::Connected { node, .. } => anyhow::ensure!(
                        node_ids.contains(node),
                        "edge {} refers to missing node {node}",
                        edge.id
                    ),
                    EdgeEnd::Free { x, y } => anyhow::ensure!(
                        x.is_finite() && y.is_finite(),
                        "edge {} has a non-finite free endpoint",
                        edge.id
                    ),
                }
            }
        }
        for group in &self.groups {
            anyhow::ensure!(ids.insert(&group.id), "duplicate canvas id {}", group.id);
            anyhow::ensure!(
                group.child_nodes.iter().all(|id| node_ids.contains(id)),
                "group {} refers to a missing node",
                group.id
            );
            anyhow::ensure!(
                group.child_edges.iter().all(|id| edge_ids.contains(id)),
                "group {} refers to a missing edge",
                group.id
            );
        }
        Ok(())
    }
}

impl Canvas {
    pub fn content_bounds(&self) -> Option<(Point, Point)> {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut include = |point: Point| {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        };
        for node in &self.nodes {
            include(Point {
                x: node.x,
                y: node.y,
            });
            include(Point {
                x: node.x + node.width,
                y: node.y + node.height,
            });
        }
        for edge in &self.edges {
            if let Some(point) = canvas_edge_endpoint(&edge.from, &self.nodes) {
                include(point);
            }
            if let Some(point) = canvas_edge_endpoint(&edge.to, &self.nodes) {
                include(point);
            }
        }
        min_x.is_finite().then_some((
            Point { x: min_x, y: min_y },
            Point {
                x: (max_x - min_x).max(1.0),
                y: (max_y - min_y).max(1.0),
            },
        ))
    }

    /// Export a portable vector snapshot without a browser canvas. DSS/static
    /// media remains a labeled frame because the persisted schema stores an
    /// authenticated UUID, not a public URL that can safely be embedded.
    pub fn to_svg(&self) -> Result<String> {
        self.validate()?;
        let (origin, size) = self.content_bounds().unwrap_or_default();
        let padding = 24.0;
        let view_x = origin.x - padding;
        let view_y = origin.y - padding;
        let width = size.x + padding * 2.0;
        let height = size.y + padding * 2.0;
        let mut output = String::new();
        writeln!(
            output,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"{view_x} {view_y} {width} {height}\">"
        )?;
        output.push_str(SVG_MARKER_DEFINITIONS);

        let mut edges = self.edges.iter().collect::<Vec<_>>();
        edges.sort_by(|left, right| {
            left.layer
                .total_cmp(&right.layer)
                .then(left.sort_order.total_cmp(&right.sort_order))
        });
        for edge in edges {
            write_canvas_edge_svg(&mut output, edge, &self.nodes)?;
        }

        let mut nodes = self.nodes.iter().collect::<Vec<_>>();
        nodes.sort_by(|left, right| {
            left.layer
                .total_cmp(&right.layer)
                .then(left.sort_order.total_cmp(&right.sort_order))
        });
        for node in nodes {
            write_canvas_node_svg(&mut output, node)?;
        }
        output.push_str("</svg>\n");
        Ok(output)
    }
}

fn validate_style(style: Option<&CanvasStyle>, owner: &str) -> Result<()> {
    let Some(style) = style else {
        return Ok(());
    };
    for (name, value) in [
        ("strokeWidth", style.stroke_width),
        ("cornerRadius", style.corner_radius),
        ("textSize", style.text_size),
    ] {
        if let Some(value) = value {
            anyhow::ensure!(
                value.is_finite() && value >= 0.0,
                "canvas item {owner} has invalid {name}"
            );
        }
    }
    if let Some(opacity) = style.opacity {
        anyhow::ensure!(
            opacity.is_finite() && (0.0..=1.0).contains(&opacity),
            "canvas item {owner} has invalid opacity"
        );
    }
    for color in [style.fill_color.as_deref(), style.stroke_color.as_deref()]
        .into_iter()
        .flatten()
    {
        anyhow::ensure!(
            color == "transparent" || valid_hex_color(color),
            "canvas item {owner} has invalid color {color}"
        );
    }
    Ok(())
}

fn valid_hex_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

const SVG_MARKER_DEFINITIONS: &str = concat!(
    "<defs>\n",
    "<marker id=\"macro-arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\"><path d=\"M 1 1 L 9 5 L 1 9\" fill=\"none\" stroke=\"context-stroke\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></marker>\n",
    "<marker id=\"macro-arrow-filled\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\"><path d=\"M 0 0 L 10 5 L 0 10 z\" fill=\"context-stroke\"/></marker>\n",
    "<marker id=\"macro-circle\" viewBox=\"0 0 10 10\" refX=\"5\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto\"><circle cx=\"5\" cy=\"5\" r=\"4\" fill=\"context-stroke\"/></marker>\n",
    "<marker id=\"macro-circle-small\" viewBox=\"0 0 10 10\" refX=\"5\" refY=\"5\" markerWidth=\"5\" markerHeight=\"5\" orient=\"auto\"><circle cx=\"5\" cy=\"5\" r=\"3\" fill=\"context-stroke\"/></marker>\n",
    "</defs>\n",
);

fn canvas_edge_endpoint(end: &EdgeEnd, nodes: &[CanvasNode]) -> Option<Point> {
    match end {
        EdgeEnd::Free { x, y } => Some(Point { x: *x, y: *y }),
        EdgeEnd::Connected { node, side } => {
            let node = nodes.iter().find(|candidate| candidate.id == *node)?;
            Some(match side {
                EdgeSide::Top => Point {
                    x: node.x + node.width / 2.0,
                    y: node.y,
                },
                EdgeSide::Right => Point {
                    x: node.x + node.width,
                    y: node.y + node.height / 2.0,
                },
                EdgeSide::Bottom => Point {
                    x: node.x + node.width / 2.0,
                    y: node.y + node.height,
                },
                EdgeSide::Left => Point {
                    x: node.x,
                    y: node.y + node.height / 2.0,
                },
            })
        }
    }
}

fn selection_bounds_for(
    canvas: &Canvas,
    selected_nodes: &HashSet<CanvasId>,
    selected_edges: &HashSet<CanvasId>,
) -> Option<(Point, Point)> {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut any = false;
    for node in &canvas.nodes {
        if selected_nodes.contains(&node.id) {
            min_x = min_x.min(node.x);
            min_y = min_y.min(node.y);
            max_x = max_x.max(node.x + node.width);
            max_y = max_y.max(node.y + node.height);
            any = true;
        }
    }
    for edge in &canvas.edges {
        if !selected_edges.contains(&edge.id) {
            continue;
        }
        for point in [
            canvas_edge_endpoint(&edge.from, &canvas.nodes),
            canvas_edge_endpoint(&edge.to, &canvas.nodes),
        ]
        .into_iter()
        .flatten()
        {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
            any = true;
        }
    }
    any.then_some((
        Point { x: min_x, y: min_y },
        Point {
            x: (max_x - min_x).max(1.0),
            y: (max_y - min_y).max(1.0),
        },
    ))
}

fn rectangles_intersect(a_min: Point, a_max: Point, b_min: Point, b_max: Point) -> bool {
    a_min.x <= b_max.x && a_max.x >= b_min.x && a_min.y <= b_max.y && a_max.y >= b_min.y
}

fn line_intersects_rect(from: Point, to: Point, min: Point, max: Point) -> bool {
    if point_in_rect(from, min, max) || point_in_rect(to, min, max) {
        return true;
    }
    let top_left = min;
    let top_right = Point { x: max.x, y: min.y };
    let bottom_right = max;
    let bottom_left = Point { x: min.x, y: max.y };
    [
        (top_left, top_right),
        (top_right, bottom_right),
        (bottom_right, bottom_left),
        (bottom_left, top_left),
    ]
    .into_iter()
    .any(|(edge_from, edge_to)| segments_intersect(from, to, edge_from, edge_to))
}

fn point_in_rect(point: Point, min: Point, max: Point) -> bool {
    point.x >= min.x && point.x <= max.x && point.y >= min.y && point.y <= max.y
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    fn orientation(a: Point, b: Point, c: Point) -> f64 {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    }
    fn on_segment(a: Point, b: Point, point: Point) -> bool {
        point.x >= a.x.min(b.x)
            && point.x <= a.x.max(b.x)
            && point.y >= a.y.min(b.y)
            && point.y <= a.y.max(b.y)
    }

    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    if ((ab_c > 0.0 && ab_d < 0.0) || (ab_c < 0.0 && ab_d > 0.0))
        && ((cd_a > 0.0 && cd_b < 0.0) || (cd_a < 0.0 && cd_b > 0.0))
    {
        return true;
    }
    const EPSILON: f64 = 1e-9;
    (ab_c.abs() <= EPSILON && on_segment(a, b, c))
        || (ab_d.abs() <= EPSILON && on_segment(a, b, d))
        || (cd_a.abs() <= EPSILON && on_segment(c, d, a))
        || (cd_b.abs() <= EPSILON && on_segment(c, d, b))
}

fn canvas_style_index(value: &Value) -> Option<u8> {
    value.as_u64().map(|value| value as u8).or_else(|| {
        value.as_str().and_then(|value| match value {
            "none" | "straight" => Some(0),
            "arrow" | "stepped" => Some(1),
            "arrow-filled" | "smooth" => Some(2),
            "circle" => Some(3),
            "circle-small" => Some(4),
            _ => None,
        })
    })
}

fn canvas_marker_url(value: &Value) -> Option<&'static str> {
    match canvas_style_index(value)? {
        1 => Some("macro-arrow"),
        2 => Some("macro-arrow-filled"),
        3 => Some("macro-circle"),
        4 => Some("macro-circle-small"),
        _ => None,
    }
}

fn write_canvas_edge_svg(
    output: &mut String,
    edge: &CanvasEdge,
    nodes: &[CanvasNode],
) -> Result<()> {
    let Some(from) = canvas_edge_endpoint(&edge.from, nodes) else {
        return Ok(());
    };
    let Some(to) = canvas_edge_endpoint(&edge.to, nodes) else {
        return Ok(());
    };
    let style = edge.style.as_ref();
    let stroke = style
        .and_then(|style| style.stroke_color.as_deref())
        .unwrap_or("#71717a");
    let stroke_width = style.and_then(|style| style.stroke_width).unwrap_or(1.5);
    let opacity = style.and_then(|style| style.opacity).unwrap_or(1.0);
    let path = match style
        .and_then(|style| style.connection_style.as_ref())
        .and_then(canvas_style_index)
        .unwrap_or(0)
    {
        1 => {
            let middle = (from.x + to.x) / 2.0;
            format!(
                "M {} {} L {middle} {} L {middle} {} L {} {}",
                from.x, from.y, from.y, to.y, to.x, to.y
            )
        }
        2 => {
            let middle = (from.x + to.x) / 2.0;
            format!(
                "M {} {} Q {middle} {} {} {}",
                from.x, from.y, from.y, to.x, to.y
            )
        }
        _ => format!("M {} {} L {} {}", from.x, from.y, to.x, to.y),
    };
    let marker_start = style
        .and_then(|style| style.from_end_style.as_ref())
        .and_then(canvas_marker_url)
        .map(|marker| format!(" marker-start=\"url(#{marker})\""))
        .unwrap_or_default();
    let marker_end = style
        .and_then(|style| style.to_end_style.as_ref())
        .and_then(canvas_marker_url)
        .map(|marker| format!(" marker-end=\"url(#{marker})\""))
        .unwrap_or_default();
    writeln!(
        output,
        "<path d=\"{path}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{stroke_width}\" opacity=\"{opacity}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"{marker_start}{marker_end}/>"
    )?;
    if let Some(label) = edge.label.as_deref().filter(|label| !label.is_empty()) {
        writeln!(
            output,
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-family=\"system-ui, sans-serif\" font-size=\"12\" fill=\"#52525b\">{}</text>",
            (from.x + to.x) / 2.0,
            (from.y + to.y) / 2.0 - 6.0,
            xml_escape(label),
        )?;
    }
    Ok(())
}

fn write_canvas_node_svg(output: &mut String, node: &CanvasNode) -> Result<()> {
    let style = node.style.as_ref();
    let fill = style
        .and_then(|style| style.fill_color.as_deref())
        .unwrap_or("#fafafa");
    let stroke = style
        .and_then(|style| style.stroke_color.as_deref())
        .unwrap_or("#a1a1aa");
    let stroke_width = style.and_then(|style| style.stroke_width).unwrap_or(1.0);
    let opacity = style.and_then(|style| style.opacity).unwrap_or(1.0);
    let radius = style.and_then(|style| style.corner_radius).unwrap_or(8.0);
    let font_size = style.and_then(|style| style.text_size).unwrap_or(14.0);
    if let CanvasNodeKind::Pencil {
        coords,
        w_scale,
        h_scale,
    } = &node.kind
    {
        if coords.is_empty() {
            return Ok(());
        }
        let mut path = String::new();
        for (index, (x, y)) in coords.iter().enumerate() {
            let command = if index == 0 { 'M' } else { 'L' };
            write!(
                path,
                "{command} {} {} ",
                node.x + x * w_scale,
                node.y + y * h_scale
            )?;
        }
        writeln!(
            output,
            "<path d=\"{path}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{stroke_width}\" opacity=\"{opacity}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
        )?;
        return Ok(());
    }

    let ellipse = matches!(
        node.kind,
        CanvasNodeKind::Shape {
            shape: ShapeType::Ellipse
        }
    );
    if ellipse {
        writeln!(
            output,
            "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{stroke_width}\" opacity=\"{opacity}\"/>",
            node.x + node.width / 2.0,
            node.y + node.height / 2.0,
            node.width / 2.0,
            node.height / 2.0,
        )?;
    } else {
        writeln!(
            output,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{radius}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{stroke_width}\" opacity=\"{opacity}\"/>",
            node.x, node.y, node.width, node.height,
        )?;
    }
    let label = node
        .label
        .clone()
        .unwrap_or_else(|| canvas_export_node_label(&node.kind));
    if !label.is_empty() {
        writeln!(
            output,
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"system-ui, sans-serif\" font-size=\"{font_size}\" fill=\"#18181b\">{}</text>",
            node.x + node.width / 2.0,
            node.y + node.height / 2.0,
            xml_escape(&label),
        )?;
    }
    Ok(())
}

fn canvas_export_node_label(kind: &CanvasNodeKind) -> String {
    match kind {
        CanvasNodeKind::Text { text, .. } => text.clone(),
        CanvasNodeKind::EntityMention { file, .. } => format!("@{file}"),
        CanvasNodeKind::Link { url } => url.clone(),
        CanvasNodeKind::Shape { .. } => String::new(),
        CanvasNodeKind::Pencil { .. } => String::new(),
        CanvasNodeKind::Image { .. } => "Image".into(),
        CanvasNodeKind::Video { .. } => "Video".into(),
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn migrate_legacy_nodes(value: &mut Value) {
    let Some(nodes) = value.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    for node in nodes {
        let Some(object) = node.as_object_mut() else {
            continue;
        };
        if object.get("type").and_then(Value::as_str) == Some("file") {
            object.insert("type".into(), Value::String("entitymention".into()));
        }
        if object.get("type").and_then(Value::as_str) == Some("entitymention")
            && !object.contains_key("entityType")
        {
            let entity_type = if object.get("isChat").and_then(Value::as_bool) == Some(true) {
                "chat"
            } else if object.get("isProject").and_then(Value::as_bool) == Some(true) {
                "project"
            } else {
                "document"
            };
            object.insert("entityType".into(), Value::String(entity_type.into()));
        }
        object.remove("isChat");
        object.remove("isRss");
        object.remove("isProject");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    HorizontalCenter,
    Right,
    Top,
    VerticalCenter,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasReconcileOutcome {
    Unchanged,
    FastForwarded,
    Merged,
    Conflict,
}

#[derive(Debug, Clone)]
pub struct CanvasModel {
    canvas: Canvas,
    saved: Canvas,
    selected_nodes: HashSet<CanvasId>,
    selected_edges: HashSet<CanvasId>,
    undo: Vec<Canvas>,
    redo: Vec<Canvas>,
    interaction_snapshot: Option<Canvas>,
    pub viewport: Viewport,
    pub dirty: bool,
}

impl CanvasModel {
    pub fn new(canvas: Canvas) -> Result<Self> {
        canvas.validate()?;
        Ok(Self {
            saved: canvas.clone(),
            canvas,
            selected_nodes: HashSet::new(),
            selected_edges: HashSet::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            interaction_snapshot: None,
            viewport: Viewport::default(),
            dirty: false,
        })
    }

    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    pub fn selected_nodes(&self) -> &HashSet<CanvasId> {
        &self.selected_nodes
    }

    pub fn selected_edges(&self) -> &HashSet<CanvasId> {
        &self.selected_edges
    }

    pub fn selected_group_ids(&self) -> HashSet<CanvasId> {
        self.canvas
            .groups
            .iter()
            .filter(|group| {
                (!group.child_nodes.is_empty()
                    && group
                        .child_nodes
                        .iter()
                        .all(|id| self.selected_nodes.contains(id)))
                    || (!group.child_edges.is_empty()
                        && group
                            .child_edges
                            .iter()
                            .all(|id| self.selected_edges.contains(id)))
            })
            .map(|group| group.id.clone())
            .collect()
    }

    pub fn selection_bounds(&self) -> Option<(Point, Point)> {
        selection_bounds_for(&self.canvas, &self.selected_nodes, &self.selected_edges)
    }

    pub fn select_all(&mut self) {
        self.selected_nodes = self
            .canvas
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect();
        self.selected_edges = self
            .canvas
            .edges
            .iter()
            .map(|edge| edge.id.clone())
            .collect();
    }

    /// Replaces the active selection with every node and connector touched by
    /// the marquee. When `additive` is true the selection present at pointer
    /// down is retained. Group membership expands a hit to the full group, as
    /// Macro's web canvas does.
    pub fn select_intersecting_rect(&mut self, from: Point, to: Point, additive: bool) {
        let min = Point {
            x: from.x.min(to.x),
            y: from.y.min(to.y),
        };
        let max = Point {
            x: from.x.max(to.x),
            y: from.y.max(to.y),
        };
        let mut nodes = if additive {
            self.selected_nodes.clone()
        } else {
            HashSet::new()
        };
        let mut edges = if additive {
            self.selected_edges.clone()
        } else {
            HashSet::new()
        };
        let mut groups = HashSet::new();

        for node in &self.canvas.nodes {
            if rectangles_intersect(
                min,
                max,
                Point {
                    x: node.x,
                    y: node.y,
                },
                Point {
                    x: node.x + node.width,
                    y: node.y + node.height,
                },
            ) {
                nodes.insert(node.id.clone());
                if let Some(group_id) = &node.group_id {
                    groups.insert(group_id.clone());
                }
            }
        }
        for edge in &self.canvas.edges {
            let Some(edge_from) = canvas_edge_endpoint(&edge.from, &self.canvas.nodes) else {
                continue;
            };
            let Some(edge_to) = canvas_edge_endpoint(&edge.to, &self.canvas.nodes) else {
                continue;
            };
            if line_intersects_rect(edge_from, edge_to, min, max) {
                edges.insert(edge.id.clone());
                if let Some(group_id) = &edge.group_id {
                    groups.insert(group_id.clone());
                }
            }
        }
        for group_id in groups {
            if let Some(group) = self.canvas.groups.iter().find(|group| group.id == group_id) {
                nodes.extend(group.child_nodes.iter().cloned());
                edges.extend(group.child_edges.iter().cloned());
            }
        }
        self.selected_nodes = nodes;
        self.selected_edges = edges;
    }

    pub fn select_only(&mut self, id: &str) -> bool {
        let Some(node) = self.canvas.nodes.iter().find(|node| node.id == id) else {
            return false;
        };
        self.selected_nodes.clear();
        self.selected_edges.clear();
        if let Some(group) = node
            .group_id
            .as_deref()
            .and_then(|group_id| self.canvas.groups.iter().find(|group| group.id == group_id))
        {
            self.selected_nodes
                .extend(group.child_nodes.iter().cloned());
            self.selected_edges
                .extend(group.child_edges.iter().cloned());
        } else {
            self.selected_nodes.insert(id.to_owned());
        }
        true
    }

    pub fn toggle_node_selection(&mut self, id: &str) -> bool {
        let Some(node) = self.canvas.nodes.iter().find(|node| node.id == id) else {
            return false;
        };
        let (nodes, edges) = if let Some(group) = node
            .group_id
            .as_deref()
            .and_then(|group_id| self.canvas.groups.iter().find(|group| group.id == group_id))
        {
            (group.child_nodes.clone(), group.child_edges.clone())
        } else {
            (vec![id.to_owned()], Vec::new())
        };
        if nodes.iter().all(|id| self.selected_nodes.contains(id)) {
            for id in nodes {
                self.selected_nodes.remove(&id);
            }
            for id in edges {
                self.selected_edges.remove(&id);
            }
        } else {
            self.selected_nodes.extend(nodes);
            self.selected_edges.extend(edges);
        }
        true
    }

    pub fn select_only_edge(&mut self, id: &str) -> bool {
        let Some(edge) = self.canvas.edges.iter().find(|edge| edge.id == id) else {
            return false;
        };
        self.selected_nodes.clear();
        self.selected_edges.clear();
        if let Some(group) = edge
            .group_id
            .as_deref()
            .and_then(|group_id| self.canvas.groups.iter().find(|group| group.id == group_id))
        {
            self.selected_nodes
                .extend(group.child_nodes.iter().cloned());
            self.selected_edges
                .extend(group.child_edges.iter().cloned());
        } else {
            self.selected_edges.insert(id.to_owned());
        }
        true
    }

    pub fn toggle_edge_selection(&mut self, id: &str) -> bool {
        if !self.canvas.edges.iter().any(|edge| edge.id == id) {
            return false;
        }
        if !self.selected_edges.remove(id) {
            self.selected_edges.insert(id.to_owned());
        }
        true
    }

    pub fn clear_selection(&mut self) {
        self.selected_nodes.clear();
        self.selected_edges.clear();
    }

    pub fn add_node(&mut self, mut node: CanvasNode) -> Result<CanvasId> {
        anyhow::ensure!(
            !self.canvas.nodes.iter().any(|other| other.id == node.id),
            "canvas node id already exists"
        );
        self.checkpoint();
        node.sort_order = self.highest_order() + 1.0;
        let id = node.id.clone();
        self.canvas.nodes.push(node);
        self.select_only(&id);
        Ok(id)
    }

    pub fn connect(&mut self, from: EdgeEnd, to: EdgeEnd) -> Result<CanvasId> {
        self.validate_edge_end(&from)?;
        self.validate_edge_end(&to)?;
        self.checkpoint();
        let id = Uuid::new_v4().to_string();
        let edge = CanvasEdge {
            id: id.clone(),
            group_id: None,
            from,
            to,
            label: None,
            style: None,
            sort_order: self.highest_order() + 1.0,
            layer: 0.0,
            extra: Map::new(),
        };
        for node_id in connected_node_ids(&edge) {
            if let Some(node) = self.canvas.nodes.iter_mut().find(|node| node.id == node_id)
                && !node.edges.contains(&id)
            {
                node.edges.push(id.clone());
            }
        }
        self.canvas.edges.push(edge);
        self.selected_edges.clear();
        self.selected_edges.insert(id.clone());
        Ok(id)
    }

    pub fn connect_selected_nodes(&mut self) -> Result<CanvasId> {
        anyhow::ensure!(
            self.selected_nodes.len() == 2,
            "select exactly two nodes to create a connector"
        );
        let mut selected = self.selected_nodes.iter().cloned().collect::<Vec<_>>();
        selected.sort();
        self.connect(
            EdgeEnd::Connected {
                node: selected[0].clone(),
                side: EdgeSide::Right,
            },
            EdgeEnd::Connected {
                node: selected[1].clone(),
                side: EdgeSide::Left,
            },
        )
    }

    pub fn update_edge_end_during_interaction(
        &mut self,
        edge_id: &str,
        update_from: bool,
        end: EdgeEnd,
    ) -> Result<bool> {
        anyhow::ensure!(
            self.interaction_snapshot.is_some(),
            "canvas connector interaction was not started"
        );
        self.validate_edge_end(&end)?;
        let edge = self
            .canvas
            .edges
            .iter_mut()
            .find(|edge| edge.id == edge_id)
            .context("canvas connector not found")?;
        let target = if update_from {
            &mut edge.from
        } else {
            &mut edge.to
        };
        if *target == end {
            return Ok(false);
        }
        *target = end;
        for node in &mut self.canvas.nodes {
            node.edges.retain(|id| id != edge_id);
        }
        let edge = self
            .canvas
            .edges
            .iter()
            .find(|edge| edge.id == edge_id)
            .expect("connector still exists");
        for node_id in connected_node_ids(edge) {
            if let Some(node) = self.canvas.nodes.iter_mut().find(|node| node.id == node_id) {
                node.edges.push(edge_id.to_owned());
            }
        }
        self.dirty = true;
        Ok(true)
    }

    pub fn update_selected_text(&mut self, text: String) -> Result<()> {
        anyhow::ensure!(
            self.selected_nodes.len() == 1,
            "select one text node to edit"
        );
        let id = self.selected_nodes.iter().next().unwrap().clone();
        let index = self
            .canvas
            .nodes
            .iter()
            .position(|node| node.id == id)
            .context("selected canvas node no longer exists")?;
        anyhow::ensure!(
            matches!(self.canvas.nodes[index].kind, CanvasNodeKind::Text { .. }),
            "selected node is not text"
        );
        self.checkpoint();
        if let CanvasNodeKind::Text {
            text: node_text, ..
        } = &mut self.canvas.nodes[index].kind
        {
            *node_text = text;
        }
        Ok(())
    }

    pub fn update_selected_node(
        &mut self,
        width: f64,
        height: f64,
        label: Option<String>,
        style: CanvasStyle,
    ) -> Result<()> {
        anyhow::ensure!(
            self.selected_nodes.len() == 1,
            "select one canvas node to edit"
        );
        anyhow::ensure!(
            width.is_finite() && height.is_finite() && width >= 0.0 && height >= 0.0,
            "invalid node size"
        );
        validate_style(Some(&style), "selection")?;
        let id = self.selected_nodes.iter().next().unwrap().clone();
        let index = self
            .canvas
            .nodes
            .iter()
            .position(|node| node.id == id)
            .context("selected canvas node no longer exists")?;
        self.checkpoint();
        let node = &mut self.canvas.nodes[index];
        node.width = width;
        node.height = height;
        node.label = label.filter(|label| !label.trim().is_empty());
        node.style = (!style.is_empty()).then_some(style);
        Ok(())
    }

    pub fn update_selected_edge(
        &mut self,
        label: Option<String>,
        style: CanvasStyle,
    ) -> Result<()> {
        anyhow::ensure!(
            self.selected_edges.len() == 1,
            "select one connector to edit"
        );
        validate_style(Some(&style), "selection")?;
        let id = self.selected_edges.iter().next().unwrap().clone();
        let index = self
            .canvas
            .edges
            .iter()
            .position(|edge| edge.id == id)
            .context("selected canvas connector no longer exists")?;
        self.checkpoint();
        let edge = &mut self.canvas.edges[index];
        edge.label = label.filter(|label| !label.trim().is_empty());
        edge.style = (!style.is_empty()).then_some(style);
        Ok(())
    }

    /// Reverse every selected connector while preserving its style. Macro's
    /// canvas treats arrowheads as endpoint styles, so swapping the endpoints
    /// reverses the visible direction without rewriting those style fields.
    pub fn reverse_selected_edges(&mut self) -> bool {
        if self.selected_edges.is_empty() {
            return false;
        }
        self.checkpoint();
        let mut changed = false;
        for edge in &mut self.canvas.edges {
            if self.selected_edges.contains(&edge.id) {
                std::mem::swap(&mut edge.from, &mut edge.to);
                changed = true;
            }
        }
        changed
    }

    pub fn add_pencil_stroke(
        &mut self,
        points: &[Point],
        style: Option<CanvasStyle>,
    ) -> Result<CanvasId> {
        anyhow::ensure!(
            points.len() >= 2,
            "a freehand stroke needs at least two points"
        );
        anyhow::ensure!(
            points
                .iter()
                .all(|point| point.x.is_finite() && point.y.is_finite()),
            "freehand stroke contains invalid coordinates"
        );
        let simplified = simplify_points(points, 0.25);
        let min_x = simplified
            .iter()
            .map(|point| point.x)
            .fold(f64::INFINITY, f64::min);
        let max_x = simplified
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = simplified
            .iter()
            .map(|point| point.y)
            .fold(f64::INFINITY, f64::min);
        let max_y = simplified
            .iter()
            .map(|point| point.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let mut node = CanvasNode::new(
            CanvasNodeKind::Pencil {
                coords: simplified
                    .into_iter()
                    .map(|point| (point.x - min_x, point.y - min_y))
                    .collect(),
                w_scale: 1.0,
                h_scale: 1.0,
            },
            min_x,
            min_y,
            max_x - min_x,
            max_y - min_y,
        );
        node.style = style;
        self.add_node(node)
    }

    pub fn flip_selected_media(&mut self, horizontal: bool) -> bool {
        let selected = self
            .canvas
            .nodes
            .iter()
            .filter(|node| self.selected_nodes.contains(&node.id))
            .filter(|node| {
                matches!(
                    node.kind,
                    CanvasNodeKind::Image { .. } | CanvasNodeKind::Video { .. }
                )
            })
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        if selected.is_empty() {
            return false;
        }
        self.checkpoint();
        for node in &mut self.canvas.nodes {
            if !selected.contains(&node.id) {
                continue;
            }
            match &mut node.kind {
                CanvasNodeKind::Image { flip_x, flip_y, .. }
                | CanvasNodeKind::Video { flip_x, flip_y, .. } => {
                    if horizontal {
                        *flip_x = !*flip_x;
                    } else {
                        *flip_y = !*flip_y;
                    }
                }
                _ => {}
            }
        }
        true
    }

    pub fn move_selected(&mut self, delta: Point) -> bool {
        if self.selected_nodes.is_empty() || (delta.x == 0.0 && delta.y == 0.0) {
            return false;
        }
        self.checkpoint();
        for node in &mut self.canvas.nodes {
            if self.selected_nodes.contains(&node.id) {
                node.x += delta.x;
                node.y += delta.y;
            }
        }
        true
    }

    pub fn begin_interaction(&mut self) {
        if self.interaction_snapshot.is_none() {
            self.interaction_snapshot = Some(self.canvas.clone());
        }
    }

    pub fn move_selected_during_interaction(&mut self, delta: Point) -> bool {
        if self.interaction_snapshot.is_none()
            || self.selected_nodes.is_empty()
            || (delta.x == 0.0 && delta.y == 0.0)
        {
            return false;
        }
        for node in &mut self.canvas.nodes {
            if self.selected_nodes.contains(&node.id) {
                node.x += delta.x;
                node.y += delta.y;
            }
        }
        self.dirty = true;
        true
    }

    pub fn commit_interaction(&mut self) -> bool {
        let Some(previous) = self.interaction_snapshot.take() else {
            return false;
        };
        if previous == self.canvas {
            return false;
        }
        self.undo.push(previous);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        true
    }

    pub fn cancel_interaction(&mut self) -> bool {
        let Some(previous) = self.interaction_snapshot.take() else {
            return false;
        };
        self.canvas = previous;
        true
    }

    pub fn resize_node(&mut self, id: &str, width: f64, height: f64) -> Result<()> {
        anyhow::ensure!(width.is_finite() && height.is_finite(), "invalid node size");
        anyhow::ensure!(
            width >= 0.0 && height >= 0.0,
            "node size cannot be negative"
        );
        let index = self
            .canvas
            .nodes
            .iter()
            .position(|node| node.id == id)
            .context("canvas node not found")?;
        self.checkpoint();
        self.canvas.nodes[index].width = width;
        self.canvas.nodes[index].height = height;
        Ok(())
    }

    pub fn resize_node_during_interaction(
        &mut self,
        id: &str,
        width: f64,
        height: f64,
    ) -> Result<bool> {
        anyhow::ensure!(width.is_finite() && height.is_finite(), "invalid node size");
        anyhow::ensure!(
            width >= 0.0 && height >= 0.0,
            "node size cannot be negative"
        );
        anyhow::ensure!(
            self.interaction_snapshot.is_some(),
            "canvas resize interaction was not started"
        );
        let node = self
            .canvas
            .nodes
            .iter_mut()
            .find(|node| node.id == id)
            .context("canvas node not found")?;
        if node.width == width && node.height == height {
            return Ok(false);
        }
        node.width = width;
        node.height = height;
        self.dirty = true;
        Ok(true)
    }

    pub fn rescale_selection_during_interaction(
        &mut self,
        origin: Point,
        size: Point,
    ) -> Result<bool> {
        anyhow::ensure!(
            [origin.x, origin.y, size.x, size.y]
                .into_iter()
                .all(f64::is_finite),
            "invalid selection bounds"
        );
        anyhow::ensure!(
            size.x >= 1.0 && size.y >= 1.0,
            "selection size must be positive"
        );
        let snapshot = self
            .interaction_snapshot
            .as_ref()
            .context("canvas resize interaction was not started")?;
        let (previous_origin, previous_size) =
            selection_bounds_for(snapshot, &self.selected_nodes, &self.selected_edges)
                .context("canvas selection has no rescalable bounds")?;
        let scale_x = size.x / previous_size.x.max(1.0);
        let scale_y = size.y / previous_size.y.max(1.0);
        let mut changed = false;

        for node in &mut self.canvas.nodes {
            if !self.selected_nodes.contains(&node.id) {
                continue;
            }
            let Some(previous) = snapshot
                .nodes
                .iter()
                .find(|previous| previous.id == node.id)
            else {
                continue;
            };
            let next_x = origin.x + (previous.x - previous_origin.x) * scale_x;
            let next_y = origin.y + (previous.y - previous_origin.y) * scale_y;
            let mut next_width = (previous.width * scale_x).max(1.0);
            let mut next_height = (previous.height * scale_y).max(1.0);
            match &previous.kind {
                CanvasNodeKind::Text {
                    follow_text_width, ..
                } => {
                    if follow_text_width.unwrap_or(false) {
                        next_width = previous.width;
                    }
                    next_height = previous.height;
                }
                CanvasNodeKind::Pencil {
                    w_scale, h_scale, ..
                } => {
                    if let CanvasNodeKind::Pencil {
                        w_scale: next_w_scale,
                        h_scale: next_h_scale,
                        ..
                    } = &mut node.kind
                    {
                        *next_w_scale = *w_scale * scale_x;
                        *next_h_scale = *h_scale * scale_y;
                    }
                }
                _ => {}
            }
            if (node.x, node.y, node.width, node.height)
                != (next_x, next_y, next_width, next_height)
            {
                node.x = next_x;
                node.y = next_y;
                node.width = next_width;
                node.height = next_height;
                changed = true;
            }
        }
        for edge in &mut self.canvas.edges {
            if !self.selected_edges.contains(&edge.id) {
                continue;
            }
            let Some(previous) = snapshot
                .edges
                .iter()
                .find(|previous| previous.id == edge.id)
            else {
                continue;
            };
            for (end, previous_end) in [
                (&mut edge.from, &previous.from),
                (&mut edge.to, &previous.to),
            ] {
                if let EdgeEnd::Free { x, y } = previous_end {
                    let next = Point {
                        x: origin.x + (*x - previous_origin.x) * scale_x,
                        y: origin.y + (*y - previous_origin.y) * scale_y,
                    };
                    if *end
                        != (EdgeEnd::Free {
                            x: next.x,
                            y: next.y,
                        })
                    {
                        *end = EdgeEnd::Free {
                            x: next.x,
                            y: next.y,
                        };
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.dirty = true;
        }
        Ok(changed)
    }

    pub fn align_selected(&mut self, alignment: Align) -> bool {
        let selected = self
            .canvas
            .nodes
            .iter()
            .filter(|node| self.selected_nodes.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>();
        if selected.len() < 2 {
            return false;
        }
        let left = selected
            .iter()
            .map(|node| node.x)
            .fold(f64::INFINITY, f64::min);
        let right = selected
            .iter()
            .map(|node| node.x + node.width)
            .fold(f64::NEG_INFINITY, f64::max);
        let top = selected
            .iter()
            .map(|node| node.y)
            .fold(f64::INFINITY, f64::min);
        let bottom = selected
            .iter()
            .map(|node| node.y + node.height)
            .fold(f64::NEG_INFINITY, f64::max);
        self.checkpoint();
        for node in &mut self.canvas.nodes {
            if !self.selected_nodes.contains(&node.id) {
                continue;
            }
            match alignment {
                Align::Left => node.x = left,
                Align::HorizontalCenter => node.x = (left + right - node.width) / 2.0,
                Align::Right => node.x = right - node.width,
                Align::Top => node.y = top,
                Align::VerticalCenter => node.y = (top + bottom - node.height) / 2.0,
                Align::Bottom => node.y = bottom - node.height,
            }
        }
        true
    }

    pub fn distribute_selected(&mut self, horizontal: bool) -> bool {
        let mut selected = self
            .canvas
            .nodes
            .iter()
            .filter(|node| self.selected_nodes.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>();
        if selected.len() < 3 {
            return false;
        }
        selected.sort_by(|a, b| {
            let a = if horizontal { a.x } else { a.y };
            let b = if horizontal { b.x } else { b.y };
            a.total_cmp(&b)
        });
        let start = if horizontal {
            selected.first().unwrap().x
        } else {
            selected.first().unwrap().y
        };
        let end = if horizontal {
            selected.last().unwrap().x
        } else {
            selected.last().unwrap().y
        };
        let interval = (end - start) / (selected.len() - 1) as f64;
        let targets = selected
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.clone(), start + interval * index as f64))
            .collect::<HashMap<_, _>>();
        self.checkpoint();
        for node in &mut self.canvas.nodes {
            let Some(target) = targets.get(&node.id) else {
                continue;
            };
            if horizontal {
                node.x = *target;
            } else {
                node.y = *target;
            }
        }
        true
    }

    pub fn group_selected(&mut self) -> Option<CanvasId> {
        if self.selected_nodes.len() + self.selected_edges.len() < 2 {
            return None;
        }
        self.checkpoint();
        let id = Uuid::new_v4().to_string();
        for node in &mut self.canvas.nodes {
            if self.selected_nodes.contains(&node.id) {
                node.group_id = Some(id.clone());
            }
        }
        for edge in &mut self.canvas.edges {
            if self.selected_edges.contains(&edge.id) {
                edge.group_id = Some(id.clone());
            }
        }
        self.canvas.groups.push(CanvasGroup {
            id: id.clone(),
            child_nodes: self.selected_nodes.iter().cloned().collect(),
            child_edges: self.selected_edges.iter().cloned().collect(),
            sort_order: self.highest_order() + 1.0,
            layer: 0.0,
            extra: Map::new(),
        });
        Some(id)
    }

    pub fn ungroup_selected(&mut self) -> bool {
        let groups = self.selected_group_ids();
        if groups.is_empty() {
            return false;
        }
        self.checkpoint();
        self.canvas
            .groups
            .retain(|group| !groups.contains(&group.id));
        for node in &mut self.canvas.nodes {
            if node
                .group_id
                .as_ref()
                .is_some_and(|group| groups.contains(group))
            {
                node.group_id = None;
            }
        }
        for edge in &mut self.canvas.edges {
            if edge
                .group_id
                .as_ref()
                .is_some_and(|group| groups.contains(group))
            {
                edge.group_id = None;
            }
        }
        true
    }

    pub fn ungroup(&mut self, group_id: &str) -> bool {
        let Some(index) = self
            .canvas
            .groups
            .iter()
            .position(|group| group.id == group_id)
        else {
            return false;
        };
        self.checkpoint();
        self.canvas.groups.remove(index);
        for node in &mut self.canvas.nodes {
            if node.group_id.as_deref() == Some(group_id) {
                node.group_id = None;
            }
        }
        for edge in &mut self.canvas.edges {
            if edge.group_id.as_deref() == Some(group_id) {
                edge.group_id = None;
            }
        }
        true
    }

    pub fn delete_selected(&mut self) -> bool {
        if self.selected_nodes.is_empty() && self.selected_edges.is_empty() {
            return false;
        }
        self.checkpoint();
        let removed_nodes = self.selected_nodes.clone();
        let mut removed_edges = self.selected_edges.clone();
        for edge in &self.canvas.edges {
            if connected_node_ids(edge)
                .iter()
                .any(|node_id| removed_nodes.contains(*node_id))
            {
                removed_edges.insert(edge.id.clone());
            }
        }
        self.canvas
            .nodes
            .retain(|node| !removed_nodes.contains(&node.id));
        self.canvas
            .edges
            .retain(|edge| !removed_edges.contains(&edge.id));
        for node in &mut self.canvas.nodes {
            node.edges.retain(|edge| !removed_edges.contains(edge));
        }
        for group in &mut self.canvas.groups {
            group
                .child_nodes
                .retain(|node| !removed_nodes.contains(node));
            group
                .child_edges
                .retain(|edge| !removed_edges.contains(edge));
        }
        self.canvas
            .groups
            .retain(|group| !group.child_nodes.is_empty() || !group.child_edges.is_empty());
        self.clear_selection();
        true
    }

    pub fn bring_selected_to_front(&mut self) -> bool {
        if self.selected_nodes.is_empty() && self.selected_edges.is_empty() {
            return false;
        }
        self.checkpoint();
        let mut order = self.highest_order() + 1.0;
        for node in &mut self.canvas.nodes {
            if self.selected_nodes.contains(&node.id) {
                node.sort_order = order;
                order += 1.0;
            }
        }
        for edge in &mut self.canvas.edges {
            if self.selected_edges.contains(&edge.id) {
                edge.sort_order = order;
                order += 1.0;
            }
        }
        true
    }

    pub fn bring_selected_forward(&mut self) -> bool {
        self.reorder_selected_one_step(true)
    }

    pub fn send_selected_to_back(&mut self) -> bool {
        if self.selected_nodes.is_empty() && self.selected_edges.is_empty() {
            return false;
        }
        self.checkpoint();
        let mut order = self
            .canvas
            .nodes
            .iter()
            .map(|node| node.sort_order)
            .chain(self.canvas.edges.iter().map(|edge| edge.sort_order))
            .fold(0.0, f64::min)
            - 1.0;
        for node in &mut self.canvas.nodes {
            if self.selected_nodes.contains(&node.id) {
                node.sort_order = order;
                order -= 1.0;
            }
        }
        for edge in &mut self.canvas.edges {
            if self.selected_edges.contains(&edge.id) {
                edge.sort_order = order;
                order -= 1.0;
            }
        }
        true
    }

    pub fn send_selected_backward(&mut self) -> bool {
        self.reorder_selected_one_step(false)
    }

    fn reorder_selected_one_step(&mut self, forward: bool) -> bool {
        #[derive(Clone, Copy)]
        enum Target {
            Node(usize),
            Edge(usize),
        }

        let mut ordered = self
            .canvas
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                (
                    node.sort_order,
                    Target::Node(index),
                    self.selected_nodes.contains(&node.id),
                )
            })
            .chain(self.canvas.edges.iter().enumerate().map(|(index, edge)| {
                (
                    edge.sort_order,
                    Target::Edge(index),
                    self.selected_edges.contains(&edge.id),
                )
            }))
            .collect::<Vec<_>>();
        ordered.sort_by(|left, right| left.0.total_cmp(&right.0));

        let mut changed = false;
        if forward {
            for index in (0..ordered.len().saturating_sub(1)).rev() {
                if ordered[index].2 && !ordered[index + 1].2 {
                    ordered.swap(index, index + 1);
                    changed = true;
                }
            }
        } else {
            for index in 1..ordered.len() {
                if ordered[index].2 && !ordered[index - 1].2 {
                    ordered.swap(index - 1, index);
                    changed = true;
                }
            }
        }
        if !changed {
            return false;
        }

        self.checkpoint();
        for (rank, (_, target, _)) in ordered.into_iter().enumerate() {
            match target {
                Target::Node(index) => self.canvas.nodes[index].sort_order = rank as f64,
                Target::Edge(index) => self.canvas.edges[index].sort_order = rank as f64,
            }
        }
        true
    }

    pub fn copy_selection_json(&self) -> Result<String> {
        let mut included_node_ids = self.selected_nodes.clone();
        for edge in self
            .canvas
            .edges
            .iter()
            .filter(|edge| self.selected_edges.contains(&edge.id))
        {
            included_node_ids.extend(connected_node_ids(edge).into_iter().map(ToOwned::to_owned));
        }
        let mut nodes = self
            .canvas
            .nodes
            .iter()
            .filter(|node| included_node_ids.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>();
        let node_ids = nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        let edges = self
            .canvas
            .edges
            .iter()
            .filter(|edge| {
                self.selected_edges.contains(&edge.id)
                    || connected_node_ids(edge)
                        .iter()
                        .all(|node| node_ids.contains(*node))
            })
            .cloned()
            .collect::<Vec<_>>();
        let edge_ids = edges.iter().map(|edge| &edge.id).collect::<HashSet<_>>();
        for node in &mut nodes {
            node.edges.retain(|edge| edge_ids.contains(edge));
        }
        anyhow::ensure!(!nodes.is_empty() || !edges.is_empty(), "nothing selected");
        let group_ids = self.selected_group_ids();
        let groups = self
            .canvas
            .groups
            .iter()
            .filter(|group| group_ids.contains(&group.id))
            .cloned()
            .collect();
        serde_json::to_string(&Canvas {
            nodes,
            edges,
            groups,
            extra: Map::new(),
        })
        .context("failed to copy canvas selection")
    }

    pub fn paste_json(&mut self, json: &str, offset: Point) -> Result<Vec<CanvasId>> {
        let mut pasted = Canvas::from_json(json.as_bytes())?;
        anyhow::ensure!(
            !pasted.nodes.is_empty() || !pasted.edges.is_empty(),
            "canvas clipboard is empty"
        );
        let mut remap = HashMap::new();
        for node in &pasted.nodes {
            remap.insert(node.id.clone(), Uuid::new_v4().to_string());
        }
        for edge in &pasted.edges {
            remap.insert(edge.id.clone(), Uuid::new_v4().to_string());
        }
        for group in &pasted.groups {
            remap.insert(group.id.clone(), Uuid::new_v4().to_string());
        }
        let highest_order = self.highest_order();
        for (index, node) in pasted.nodes.iter_mut().enumerate() {
            node.id = remap[&node.id].clone();
            node.group_id = node.group_id.as_ref().and_then(|id| remap.get(id).cloned());
            node.x += offset.x;
            node.y += offset.y;
            node.sort_order = highest_order + index as f64 + 1.0;
            node.edges = node
                .edges
                .iter()
                .filter_map(|id| remap.get(id).cloned())
                .collect();
        }
        for (index, edge) in pasted.edges.iter_mut().enumerate() {
            edge.id = remap[&edge.id].clone();
            edge.group_id = edge.group_id.as_ref().and_then(|id| remap.get(id).cloned());
            remap_edge_end(&mut edge.from, &remap)?;
            remap_edge_end(&mut edge.to, &remap)?;
            edge.sort_order = highest_order + pasted.nodes.len() as f64 + index as f64 + 1.0;
        }
        for group in &mut pasted.groups {
            group.id = remap[&group.id].clone();
            group.child_nodes = group
                .child_nodes
                .iter()
                .filter_map(|id| remap.get(id).cloned())
                .collect();
            group.child_edges = group
                .child_edges
                .iter()
                .filter_map(|id| remap.get(id).cloned())
                .collect();
        }
        let pasted_ids = pasted
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        self.checkpoint();
        self.canvas.nodes.extend(pasted.nodes);
        self.canvas.edges.extend(pasted.edges);
        self.canvas.groups.extend(pasted.groups);
        self.selected_nodes = pasted_ids.iter().cloned().collect();
        self.selected_edges.clear();
        self.canvas.validate()?;
        Ok(pasted_ids)
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.canvas, previous));
        self.clear_selection();
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.canvas, next));
        self.clear_selection();
        self.dirty = true;
        true
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.canvas.clone();
        self.dirty = false;
    }

    pub fn reconcile_remote(&mut self, remote: &Canvas) -> Result<CanvasReconcileOutcome> {
        remote.validate()?;
        if self.canvas == *remote {
            self.saved = remote.clone();
            self.dirty = false;
            return Ok(CanvasReconcileOutcome::Unchanged);
        }
        if self.canvas == self.saved {
            self.canvas = remote.clone();
            self.saved = remote.clone();
            self.undo.clear();
            self.redo.clear();
            self.clear_selection();
            self.dirty = false;
            return Ok(CanvasReconcileOutcome::FastForwarded);
        }
        if *remote == self.saved {
            return Ok(CanvasReconcileOutcome::Unchanged);
        }
        let Some(merged) = merge_canvas(&self.saved, &self.canvas, remote) else {
            return Ok(CanvasReconcileOutcome::Conflict);
        };
        merged.validate()?;
        self.canvas = merged;
        self.saved = remote.clone();
        self.redo.clear();
        self.clear_selection();
        self.dirty = self.canvas != self.saved;
        Ok(CanvasReconcileOutcome::Merged)
    }

    pub fn fit_to_view(&mut self, width: f64, height: f64, padding: f64) {
        let Some((origin, size)) = self.content_bounds() else {
            self.viewport = Viewport::default();
            return;
        };
        self.viewport.fit_bounds(
            origin,
            size,
            Point {
                x: width,
                y: height,
            },
            padding,
        );
    }

    pub fn content_bounds(&self) -> Option<(Point, Point)> {
        self.canvas.content_bounds()
    }

    fn checkpoint(&mut self) {
        self.interaction_snapshot = None;
        self.undo.push(self.canvas.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    fn highest_order(&self) -> f64 {
        self.canvas
            .nodes
            .iter()
            .map(|node| node.sort_order)
            .chain(self.canvas.edges.iter().map(|edge| edge.sort_order))
            .fold(0.0, f64::max)
    }

    fn validate_edge_end(&self, end: &EdgeEnd) -> Result<()> {
        if let EdgeEnd::Connected { node, .. } = end {
            anyhow::ensure!(
                self.canvas
                    .nodes
                    .iter()
                    .any(|candidate| candidate.id == *node),
                "cannot connect to missing node {node}"
            );
        }
        Ok(())
    }
}

impl CanvasStyle {
    pub fn is_empty(&self) -> bool {
        self.fill_color.is_none()
            && self.stroke_color.is_none()
            && self.stroke_width.is_none()
            && self.corner_radius.is_none()
            && self.opacity.is_none()
            && self.text_size.is_none()
            && self.from_end_style.is_none()
            && self.to_end_style.is_none()
            && self.connection_style.is_none()
            && self.imported_color.is_none()
            && self.extra.is_empty()
    }
}

fn simplify_points(points: &[Point], epsilon: f64) -> Vec<Point> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let first = points[0];
    let last = points[points.len() - 1];
    let mut split_index = 0;
    let mut largest_distance = 0.0;
    for (index, point) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let distance = perpendicular_distance(*point, first, last);
        if distance > largest_distance {
            split_index = index;
            largest_distance = distance;
        }
    }
    if largest_distance <= epsilon {
        return vec![first, last];
    }
    let mut left = simplify_points(&points[..=split_index], epsilon);
    let right = simplify_points(&points[split_index..], epsilon);
    left.pop();
    left.extend(right);
    left
}

fn perpendicular_distance(point: Point, start: Point, end: Point) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    if dx == 0.0 && dy == 0.0 {
        return ((point.x - start.x).powi(2) + (point.y - start.y).powi(2)).sqrt();
    }
    ((dy * point.x - dx * point.y + end.x * start.y - end.y * start.x).abs())
        / (dx * dx + dy * dy).sqrt()
}

fn merge_canvas(base: &Canvas, local: &Canvas, remote: &Canvas) -> Option<Canvas> {
    Some(Canvas {
        nodes: merge_canvas_entities(
            &base.nodes,
            &local.nodes,
            &remote.nodes,
            |node| &node.id,
            merge_canvas_node,
        )?,
        edges: merge_canvas_entities(
            &base.edges,
            &local.edges,
            &remote.edges,
            |edge| &edge.id,
            merge_canvas_edge,
        )?,
        groups: merge_canvas_entities(
            &base.groups,
            &local.groups,
            &remote.groups,
            |group| &group.id,
            merge_canvas_group,
        )?,
        extra: merge_canvas_map(&base.extra, &local.extra, &remote.extra)?,
    })
}

fn merge_canvas_entities<T, F, M>(
    base: &[T],
    local: &[T],
    remote: &[T],
    id: F,
    merge_fields: M,
) -> Option<Vec<T>>
where
    T: Clone + PartialEq,
    F: Fn(&T) -> &str,
    M: Fn(&T, &T, &T) -> Option<T>,
{
    let mut order = remote
        .iter()
        .chain(local.iter())
        .map(|item| id(item).to_owned())
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    order.retain(|item_id| seen.insert(item_id.clone()));
    let base = base
        .iter()
        .map(|item| (id(item), item))
        .collect::<HashMap<_, _>>();
    let local = local
        .iter()
        .map(|item| (id(item), item))
        .collect::<HashMap<_, _>>();
    let remote = remote
        .iter()
        .map(|item| (id(item), item))
        .collect::<HashMap<_, _>>();
    let mut merged = Vec::new();
    for item_id in order {
        let base = base.get(item_id.as_str()).copied();
        let local = local.get(item_id.as_str()).copied();
        let remote = remote.get(item_id.as_str()).copied();
        if local == remote {
            if let Some(item) = local {
                merged.push(item.clone());
            }
        } else if local == base {
            if let Some(item) = remote {
                merged.push(item.clone());
            }
        } else if remote == base {
            if let Some(item) = local {
                merged.push(item.clone());
            }
        } else if let (Some(base), Some(local), Some(remote)) = (base, local, remote) {
            merged.push(merge_fields(base, local, remote)?);
        } else {
            // A deletion racing a modification, or two different creations
            // with the same identifier, needs an explicit user decision.
            return None;
        }
    }
    Some(merged)
}

fn merge_canvas_node(
    base: &CanvasNode,
    local: &CanvasNode,
    remote: &CanvasNode,
) -> Option<CanvasNode> {
    Some(CanvasNode {
        id: base.id.clone(),
        group_id: merge_canvas_field(&base.group_id, &local.group_id, &remote.group_id)?,
        x: merge_canvas_field(&base.x, &local.x, &remote.x)?,
        y: merge_canvas_field(&base.y, &local.y, &remote.y)?,
        width: merge_canvas_field(&base.width, &local.width, &remote.width)?,
        height: merge_canvas_field(&base.height, &local.height, &remote.height)?,
        edges: merge_canvas_id_list(&base.edges, &local.edges, &remote.edges),
        style: merge_canvas_style_option(
            base.style.as_ref(),
            local.style.as_ref(),
            remote.style.as_ref(),
        )?,
        sort_order: merge_canvas_field(&base.sort_order, &local.sort_order, &remote.sort_order)?,
        layer: merge_canvas_field(&base.layer, &local.layer, &remote.layer)?,
        label: merge_canvas_field(&base.label, &local.label, &remote.label)?,
        kind: merge_canvas_node_kind(&base.kind, &local.kind, &remote.kind)?,
        extra: merge_canvas_map(&base.extra, &local.extra, &remote.extra)?,
    })
}

fn merge_canvas_edge(
    base: &CanvasEdge,
    local: &CanvasEdge,
    remote: &CanvasEdge,
) -> Option<CanvasEdge> {
    Some(CanvasEdge {
        id: base.id.clone(),
        group_id: merge_canvas_field(&base.group_id, &local.group_id, &remote.group_id)?,
        from: merge_canvas_field(&base.from, &local.from, &remote.from)?,
        to: merge_canvas_field(&base.to, &local.to, &remote.to)?,
        label: merge_canvas_field(&base.label, &local.label, &remote.label)?,
        style: merge_canvas_style_option(
            base.style.as_ref(),
            local.style.as_ref(),
            remote.style.as_ref(),
        )?,
        sort_order: merge_canvas_field(&base.sort_order, &local.sort_order, &remote.sort_order)?,
        layer: merge_canvas_field(&base.layer, &local.layer, &remote.layer)?,
        extra: merge_canvas_map(&base.extra, &local.extra, &remote.extra)?,
    })
}

fn merge_canvas_group(
    base: &CanvasGroup,
    local: &CanvasGroup,
    remote: &CanvasGroup,
) -> Option<CanvasGroup> {
    Some(CanvasGroup {
        id: base.id.clone(),
        child_nodes: merge_canvas_id_list(
            &base.child_nodes,
            &local.child_nodes,
            &remote.child_nodes,
        ),
        child_edges: merge_canvas_id_list(
            &base.child_edges,
            &local.child_edges,
            &remote.child_edges,
        ),
        sort_order: merge_canvas_field(&base.sort_order, &local.sort_order, &remote.sort_order)?,
        layer: merge_canvas_field(&base.layer, &local.layer, &remote.layer)?,
        extra: merge_canvas_map(&base.extra, &local.extra, &remote.extra)?,
    })
}

fn merge_canvas_style_option(
    base: Option<&CanvasStyle>,
    local: Option<&CanvasStyle>,
    remote: Option<&CanvasStyle>,
) -> Option<Option<CanvasStyle>> {
    if local == remote {
        return Some(local.cloned());
    }
    if local == base {
        return Some(remote.cloned());
    }
    if remote == base {
        return Some(local.cloned());
    }
    match (base, local, remote) {
        (Some(base), Some(local), Some(remote)) => {
            Some(Some(merge_canvas_style(base, local, remote)?))
        }
        // Concurrently creating a style from no style can still merge cleanly:
        // the default style is the common base for both edits.
        (None, Some(local), Some(remote)) => Some(Some(merge_canvas_style(
            &CanvasStyle::default(),
            local,
            remote,
        )?)),
        _ => None,
    }
}

fn merge_canvas_style(
    base: &CanvasStyle,
    local: &CanvasStyle,
    remote: &CanvasStyle,
) -> Option<CanvasStyle> {
    Some(CanvasStyle {
        fill_color: merge_canvas_field(&base.fill_color, &local.fill_color, &remote.fill_color)?,
        stroke_color: merge_canvas_field(
            &base.stroke_color,
            &local.stroke_color,
            &remote.stroke_color,
        )?,
        stroke_width: merge_canvas_field(
            &base.stroke_width,
            &local.stroke_width,
            &remote.stroke_width,
        )?,
        corner_radius: merge_canvas_field(
            &base.corner_radius,
            &local.corner_radius,
            &remote.corner_radius,
        )?,
        opacity: merge_canvas_field(&base.opacity, &local.opacity, &remote.opacity)?,
        text_size: merge_canvas_field(&base.text_size, &local.text_size, &remote.text_size)?,
        from_end_style: merge_canvas_field(
            &base.from_end_style,
            &local.from_end_style,
            &remote.from_end_style,
        )?,
        to_end_style: merge_canvas_field(
            &base.to_end_style,
            &local.to_end_style,
            &remote.to_end_style,
        )?,
        connection_style: merge_canvas_field(
            &base.connection_style,
            &local.connection_style,
            &remote.connection_style,
        )?,
        imported_color: merge_canvas_field(
            &base.imported_color,
            &local.imported_color,
            &remote.imported_color,
        )?,
        extra: merge_canvas_map(&base.extra, &local.extra, &remote.extra)?,
    })
}

fn merge_canvas_node_kind(
    base: &CanvasNodeKind,
    local: &CanvasNodeKind,
    remote: &CanvasNodeKind,
) -> Option<CanvasNodeKind> {
    if local == remote {
        return Some(local.clone());
    }
    if local == base {
        return Some(remote.clone());
    }
    if remote == base {
        return Some(local.clone());
    }
    match (base, local, remote) {
        (
            CanvasNodeKind::Text {
                text: base_text,
                follow_text_width: base_follow,
            },
            CanvasNodeKind::Text {
                text: local_text,
                follow_text_width: local_follow,
            },
            CanvasNodeKind::Text {
                text: remote_text,
                follow_text_width: remote_follow,
            },
        ) => Some(CanvasNodeKind::Text {
            text: merge_canvas_field(base_text, local_text, remote_text)?,
            follow_text_width: merge_canvas_field(base_follow, local_follow, remote_follow)?,
        }),
        (
            CanvasNodeKind::EntityMention {
                entity_type: base_type,
                file: base_file,
                subpath: base_subpath,
                mention_uuid: base_uuid,
            },
            CanvasNodeKind::EntityMention {
                entity_type: local_type,
                file: local_file,
                subpath: local_subpath,
                mention_uuid: local_uuid,
            },
            CanvasNodeKind::EntityMention {
                entity_type: remote_type,
                file: remote_file,
                subpath: remote_subpath,
                mention_uuid: remote_uuid,
            },
        ) => Some(CanvasNodeKind::EntityMention {
            entity_type: merge_canvas_field(base_type, local_type, remote_type)?,
            file: merge_canvas_field(base_file, local_file, remote_file)?,
            subpath: merge_canvas_field(base_subpath, local_subpath, remote_subpath)?,
            mention_uuid: merge_canvas_field(base_uuid, local_uuid, remote_uuid)?,
        }),
        (
            CanvasNodeKind::Pencil {
                coords: base_coords,
                w_scale: base_w,
                h_scale: base_h,
            },
            CanvasNodeKind::Pencil {
                coords: local_coords,
                w_scale: local_w,
                h_scale: local_h,
            },
            CanvasNodeKind::Pencil {
                coords: remote_coords,
                w_scale: remote_w,
                h_scale: remote_h,
            },
        ) => Some(CanvasNodeKind::Pencil {
            coords: merge_canvas_field(base_coords, local_coords, remote_coords)?,
            w_scale: merge_canvas_field(base_w, local_w, remote_w)?,
            h_scale: merge_canvas_field(base_h, local_h, remote_h)?,
        }),
        (
            CanvasNodeKind::Image {
                status: base_status,
                uuid: base_uuid,
                flip_x: base_flip_x,
                flip_y: base_flip_y,
            },
            CanvasNodeKind::Image {
                status: local_status,
                uuid: local_uuid,
                flip_x: local_flip_x,
                flip_y: local_flip_y,
            },
            CanvasNodeKind::Image {
                status: remote_status,
                uuid: remote_uuid,
                flip_x: remote_flip_x,
                flip_y: remote_flip_y,
            },
        ) => Some(CanvasNodeKind::Image {
            status: merge_canvas_field(base_status, local_status, remote_status)?,
            uuid: merge_canvas_field(base_uuid, local_uuid, remote_uuid)?,
            flip_x: merge_canvas_field(base_flip_x, local_flip_x, remote_flip_x)?,
            flip_y: merge_canvas_field(base_flip_y, local_flip_y, remote_flip_y)?,
        }),
        (
            CanvasNodeKind::Video {
                status: base_status,
                uuid: base_uuid,
                flip_x: base_flip_x,
                flip_y: base_flip_y,
            },
            CanvasNodeKind::Video {
                status: local_status,
                uuid: local_uuid,
                flip_x: local_flip_x,
                flip_y: local_flip_y,
            },
            CanvasNodeKind::Video {
                status: remote_status,
                uuid: remote_uuid,
                flip_x: remote_flip_x,
                flip_y: remote_flip_y,
            },
        ) => Some(CanvasNodeKind::Video {
            status: merge_canvas_field(base_status, local_status, remote_status)?,
            uuid: merge_canvas_field(base_uuid, local_uuid, remote_uuid)?,
            flip_x: merge_canvas_field(base_flip_x, local_flip_x, remote_flip_x)?,
            flip_y: merge_canvas_field(base_flip_y, local_flip_y, remote_flip_y)?,
        }),
        // Link and shape nodes each have one editable payload field, so reaching
        // this branch means both peers changed that field differently.
        _ => None,
    }
}

fn merge_canvas_field<T: Clone + PartialEq>(base: &T, local: &T, remote: &T) -> Option<T> {
    if local == remote {
        Some(local.clone())
    } else if local == base {
        Some(remote.clone())
    } else if remote == base {
        Some(local.clone())
    } else {
        None
    }
}

fn merge_canvas_id_list(
    base: &[CanvasId],
    local: &[CanvasId],
    remote: &[CanvasId],
) -> Vec<CanvasId> {
    let base = base.iter().collect::<HashSet<_>>();
    let local = local.iter().collect::<HashSet<_>>();
    let remote = remote.iter().collect::<HashSet<_>>();
    let mut order = remote
        .iter()
        .chain(local.iter())
        .copied()
        .cloned()
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    order.retain(|id| seen.insert(id.clone()));
    order.retain(|id| {
        let was_present = base.contains(id);
        let local_present = local.contains(id);
        let remote_present = remote.contains(id);
        local_present == remote_present && local_present
            || local_present == was_present && remote_present
            || remote_present == was_present && local_present
    });
    order
}

fn merge_canvas_value<'a, T: PartialEq>(
    base: Option<&'a T>,
    local: Option<&'a T>,
    remote: Option<&'a T>,
) -> Option<Option<&'a T>> {
    if local == remote {
        return Some(local);
    }
    if local == base {
        return Some(remote);
    }
    if remote == base {
        return Some(local);
    }
    None
}

fn merge_canvas_map(
    base: &Map<String, Value>,
    local: &Map<String, Value>,
    remote: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut keys = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    let mut merged = Map::new();
    for key in keys {
        if let Some(value) = merge_canvas_value(base.get(key), local.get(key), remote.get(key))? {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(merged)
}

fn connected_node_ids(edge: &CanvasEdge) -> Vec<&str> {
    [&edge.from, &edge.to]
        .into_iter()
        .filter_map(|end| match end {
            EdgeEnd::Connected { node, .. } => Some(node.as_str()),
            EdgeEnd::Free { .. } => None,
        })
        .collect()
}

fn remap_edge_end(end: &mut EdgeEnd, remap: &HashMap<CanvasId, CanvasId>) -> Result<()> {
    if let EdgeEnd::Connected { node, .. } = end {
        *node = remap
            .get(node)
            .cloned()
            .context("pasted connector refers to a node outside the selection")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_macro_schema_and_unknown_fields() {
        let input = serde_json::to_vec(&json!({
            "nodes": [{
                "id": "node-1", "type": "text", "x": 1, "y": 2,
                "width": 120, "height": 40, "text": "Hello", "futureField": {"v": 1}
            }],
            "edges": [],
            "futureCanvasField": true
        }))
        .unwrap();
        let canvas = Canvas::from_json(&input).unwrap();
        let output: Value = serde_json::from_slice(&canvas.to_json().unwrap()).unwrap();
        assert_eq!(output["nodes"][0]["futureField"]["v"], 1);
        assert_eq!(output["futureCanvasField"], true);
    }

    #[test]
    fn migrates_legacy_file_mentions() {
        let input = serde_json::to_vec(&json!({
            "nodes": [{
                "id": "node-1", "type": "file", "x": 0, "y": 0,
                "width": 100, "height": 50, "file": "doc-1", "isProject": true
            }]
        }))
        .unwrap();
        let canvas = Canvas::from_json(&input).unwrap();
        assert!(matches!(
            canvas.nodes[0].kind,
            CanvasNodeKind::EntityMention {
                entity_type: CanvasMentionType::Project,
                ..
            }
        ));
    }

    #[test]
    fn edits_group_connect_and_undo_without_dangling_edges() {
        let mut model = CanvasModel::new(Canvas::default()).unwrap();
        let first = model.add_node(CanvasNode::text("A", 10.0, 20.0)).unwrap();
        let second = model
            .add_node(CanvasNode::shape(ShapeType::Ellipse, 200.0, 20.0))
            .unwrap();
        model.toggle_node_selection(&first);
        let edge = model
            .connect(
                EdgeEnd::Connected {
                    node: first.clone(),
                    side: EdgeSide::Right,
                },
                EdgeEnd::Connected {
                    node: second.clone(),
                    side: EdgeSide::Left,
                },
            )
            .unwrap();
        model.select_only(&first);
        model.toggle_node_selection(&second);
        let group = model.group_selected().unwrap();
        assert_eq!(model.canvas().groups[0].id, group);
        model.select_only(&first);
        assert!(model.delete_selected());
        assert!(
            !model
                .canvas()
                .edges
                .iter()
                .any(|candidate| candidate.id == edge)
        );
        assert!(model.undo());
        assert!(
            model
                .canvas()
                .edges
                .iter()
                .any(|candidate| candidate.id == edge)
        );
        model.canvas().validate().unwrap();
    }

    #[test]
    fn selected_entities_move_one_z_layer_without_jumping_to_an_extreme() {
        let mut first = CanvasNode::text("First", 0.0, 0.0);
        first.id = "first".into();
        first.sort_order = 0.0;
        let mut middle = CanvasNode::text("Middle", 0.0, 0.0);
        middle.id = "middle".into();
        middle.sort_order = 1.0;
        let mut last = CanvasNode::text("Last", 0.0, 0.0);
        last.id = "last".into();
        last.sort_order = 2.0;
        let mut model = CanvasModel::new(Canvas {
            nodes: vec![first, middle, last],
            ..Default::default()
        })
        .unwrap();
        assert!(model.select_only("middle"));

        assert!(model.bring_selected_forward());
        let order = |id: &str, model: &CanvasModel| {
            model
                .canvas()
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap()
                .sort_order
        };
        assert!(order("middle", &model) > order("last", &model));
        assert!(!model.bring_selected_forward());

        assert!(model.send_selected_backward());
        assert!(order("middle", &model) < order("last", &model));
        assert!(order("middle", &model) > order("first", &model));
        assert!(model.undo());
        assert!(order("middle", &model) > order("last", &model));
    }

    #[test]
    fn selected_connectors_reverse_as_one_undoable_operation() {
        let from = EdgeEnd::Connected {
            node: "first".into(),
            side: EdgeSide::Right,
        };
        let to = EdgeEnd::Free { x: 250.0, y: 75.0 };
        let edge = CanvasEdge {
            id: "edge".into(),
            group_id: None,
            from: from.clone(),
            to: to.clone(),
            label: Some("depends on".into()),
            style: Some(CanvasStyle {
                from_end_style: Some(Value::from(0)),
                to_end_style: Some(Value::from(2)),
                ..Default::default()
            }),
            sort_order: 0.0,
            layer: 0.0,
            extra: Map::new(),
        };
        let mut first = CanvasNode::text("First", 0.0, 0.0);
        first.id = "first".into();
        first.edges.push("edge".into());
        let mut model = CanvasModel::new(Canvas {
            nodes: vec![first],
            edges: vec![edge],
            ..Default::default()
        })
        .unwrap();

        assert!(model.select_only_edge("edge"));
        assert!(model.reverse_selected_edges());
        let reversed = &model.canvas().edges[0];
        assert_eq!(reversed.from, to);
        assert_eq!(reversed.to, from);
        assert_eq!(
            reversed
                .style
                .as_ref()
                .and_then(|style| style.to_end_style.as_ref()),
            Some(&Value::from(2))
        );
        assert_eq!(model.canvas().nodes[0].edges, ["edge"]);

        assert!(model.undo());
        let original = &model.canvas().edges[0];
        assert!(matches!(original.from, EdgeEnd::Connected { .. }));
        assert!(matches!(original.to, EdgeEnd::Free { .. }));
    }

    #[test]
    fn pointer_resize_coalesces_into_one_undo_checkpoint() {
        let mut node = CanvasNode::shape(ShapeType::Rectangle, 0.0, 0.0);
        node.id = "node".into();
        let original_size = (node.width, node.height);
        let mut model = CanvasModel::new(Canvas {
            nodes: vec![node],
            ..Default::default()
        })
        .unwrap();

        model.begin_interaction();
        assert!(
            model
                .resize_node_during_interaction("node", 200.0, 120.0)
                .unwrap()
        );
        assert!(
            model
                .resize_node_during_interaction("node", 220.0, 140.0)
                .unwrap()
        );
        assert!(model.commit_interaction());
        assert!(model.undo());
        let node = &model.canvas().nodes[0];
        assert_eq!((node.width, node.height), original_size);
    }

    #[test]
    fn selection_rescale_moves_nodes_and_free_edges_as_one_undo_step() {
        let mut first = CanvasNode::shape(ShapeType::Rectangle, 0.0, 0.0);
        first.id = "first".into();
        first.width = 100.0;
        first.height = 50.0;
        let mut second = CanvasNode::shape(ShapeType::Ellipse, 200.0, 100.0);
        second.id = "second".into();
        second.width = 100.0;
        second.height = 100.0;
        let canvas = Canvas {
            nodes: vec![first, second],
            edges: vec![CanvasEdge {
                id: "free".into(),
                group_id: None,
                from: EdgeEnd::Free { x: 0.0, y: 200.0 },
                to: EdgeEnd::Free { x: 300.0, y: 200.0 },
                label: None,
                style: None,
                sort_order: 3.0,
                layer: 0.0,
                extra: Map::new(),
            }],
            ..Default::default()
        };
        let mut model = CanvasModel::new(canvas).unwrap();
        model.select_all();
        assert_eq!(
            model.selection_bounds(),
            Some((Point { x: 0.0, y: 0.0 }, Point { x: 300.0, y: 200.0 }))
        );
        model.begin_interaction();
        assert!(
            model
                .rescale_selection_during_interaction(
                    Point { x: 10.0, y: 20.0 },
                    Point { x: 600.0, y: 400.0 },
                )
                .unwrap()
        );
        assert!(model.commit_interaction());
        let second = model
            .canvas()
            .nodes
            .iter()
            .find(|node| node.id == "second")
            .unwrap();
        assert_eq!(
            (second.x, second.y, second.width, second.height),
            (410.0, 220.0, 200.0, 200.0)
        );
        assert_eq!(
            model.canvas().edges[0].to,
            EdgeEnd::Free { x: 610.0, y: 420.0 }
        );
        assert!(model.undo());
        assert_eq!(model.canvas().nodes[1].x, 200.0);
    }

    #[test]
    fn connector_endpoint_drag_reconciles_node_edge_indexes_and_undoes_once() {
        let mut model = CanvasModel::new(Canvas::default()).unwrap();
        let first = model.add_node(CanvasNode::text("A", 0.0, 0.0)).unwrap();
        let second = model.add_node(CanvasNode::text("B", 200.0, 0.0)).unwrap();
        let edge = model
            .connect(
                EdgeEnd::Connected {
                    node: first.clone(),
                    side: EdgeSide::Right,
                },
                EdgeEnd::Connected {
                    node: second.clone(),
                    side: EdgeSide::Left,
                },
            )
            .unwrap();
        model.begin_interaction();
        assert!(
            model
                .update_edge_end_during_interaction(
                    &edge,
                    false,
                    EdgeEnd::Free { x: 400.0, y: 50.0 },
                )
                .unwrap()
        );
        assert!(model.commit_interaction());
        assert!(
            !model
                .canvas()
                .nodes
                .iter()
                .find(|node| node.id == second)
                .unwrap()
                .edges
                .contains(&edge)
        );
        assert!(model.undo());
        assert!(
            model
                .canvas()
                .nodes
                .iter()
                .find(|node| node.id == second)
                .unwrap()
                .edges
                .contains(&edge)
        );
    }

    #[test]
    fn zoom_preserves_the_canvas_point_under_the_pointer() {
        let mut viewport = Viewport::default();
        viewport.pan(Point { x: 20.0, y: 30.0 });
        let pointer = Point { x: 120.0, y: 130.0 };
        let before = viewport.screen_to_canvas(pointer);
        viewport.zoom_at(2.0, pointer);
        assert_eq!(viewport.screen_to_canvas(pointer), before);
    }

    #[test]
    fn marquee_selection_expands_groups_and_can_accumulate() {
        let mut first = CanvasNode::shape(ShapeType::Rectangle, 0.0, 0.0);
        first.id = "first".into();
        first.group_id = Some("group".into());
        let mut second = CanvasNode::shape(ShapeType::Rectangle, 200.0, 0.0);
        second.id = "second".into();
        second.group_id = Some("group".into());
        let mut outside = CanvasNode::text("Outside", 500.0, 500.0);
        outside.id = "outside".into();
        let canvas = Canvas {
            nodes: vec![first, second, outside],
            edges: vec![CanvasEdge {
                id: "edge".into(),
                group_id: Some("group".into()),
                from: EdgeEnd::Connected {
                    node: "first".into(),
                    side: EdgeSide::Right,
                },
                to: EdgeEnd::Connected {
                    node: "second".into(),
                    side: EdgeSide::Left,
                },
                label: None,
                style: None,
                sort_order: 2.0,
                layer: 0.0,
                extra: Map::new(),
            }],
            groups: vec![CanvasGroup {
                id: "group".into(),
                child_nodes: vec!["first".into(), "second".into()],
                child_edges: vec!["edge".into()],
                sort_order: 3.0,
                layer: 0.0,
                extra: Map::new(),
            }],
            extra: Map::new(),
        };
        let mut model = CanvasModel::new(canvas).unwrap();
        model.select_intersecting_rect(
            Point { x: -5.0, y: -5.0 },
            Point { x: 40.0, y: 40.0 },
            false,
        );
        assert_eq!(
            model.selected_nodes(),
            &HashSet::from(["first".into(), "second".into()])
        );
        assert_eq!(model.selected_edges(), &HashSet::from(["edge".into()]));

        model.select_only("outside");
        model.select_intersecting_rect(
            Point { x: -5.0, y: -5.0 },
            Point { x: 40.0, y: 40.0 },
            true,
        );
        assert_eq!(model.selected_nodes().len(), 3);
        assert!(model.selected_nodes().contains("outside"));
    }

    #[test]
    fn rejects_dangling_connections() {
        let canvas = Canvas {
            edges: vec![CanvasEdge {
                id: "edge-1".into(),
                group_id: None,
                from: EdgeEnd::Connected {
                    node: "missing".into(),
                    side: EdgeSide::Right,
                },
                to: EdgeEnd::Free { x: 1.0, y: 2.0 },
                label: None,
                style: None,
                sort_order: 0.0,
                layer: 0.0,
                extra: Map::new(),
            }],
            ..Default::default()
        };
        assert!(canvas.validate().is_err());
    }

    #[test]
    fn clipboard_remaps_nodes_and_connectors() {
        let mut model = CanvasModel::new(Canvas::default()).unwrap();
        let first = model.add_node(CanvasNode::text("A", 0.0, 0.0)).unwrap();
        let second = model.add_node(CanvasNode::text("B", 100.0, 0.0)).unwrap();
        model.toggle_node_selection(&first);
        model
            .connect(
                EdgeEnd::Connected {
                    node: first,
                    side: EdgeSide::Right,
                },
                EdgeEnd::Connected {
                    node: second,
                    side: EdgeSide::Left,
                },
            )
            .unwrap();
        let copied = model.copy_selection_json().unwrap();
        let pasted = model
            .paste_json(&copied, Point { x: 20.0, y: 30.0 })
            .unwrap();
        assert_eq!(pasted.len(), 2);
        assert_eq!(model.canvas().nodes.len(), 4);
        assert_eq!(model.canvas().edges.len(), 2);
        model.canvas().validate().unwrap();
    }

    #[test]
    fn grouped_selection_moves_and_round_trips_as_one_unit() {
        let mut model = CanvasModel::new(Canvas::default()).unwrap();
        let first = model.add_node(CanvasNode::text("A", 0.0, 0.0)).unwrap();
        let _second = model.add_node(CanvasNode::text("B", 100.0, 0.0)).unwrap();
        model.toggle_node_selection(&first);
        let group = model.group_selected().unwrap();

        model.clear_selection();
        assert!(model.select_only(&first));
        assert_eq!(model.selected_nodes().len(), 2);
        assert!(model.selected_group_ids().contains(&group));
        assert!(model.move_selected(Point { x: 10.0, y: 20.0 }));
        assert_eq!(model.canvas().nodes[0].x, 10.0);
        assert_eq!(model.canvas().nodes[1].x, 110.0);

        let copied = model.copy_selection_json().unwrap();
        let pasted = model
            .paste_json(&copied, Point { x: 25.0, y: 25.0 })
            .unwrap();
        assert_eq!(pasted.len(), 2);
        assert_eq!(model.canvas().groups.len(), 2);
        model.canvas().validate().unwrap();
    }

    #[test]
    fn freehand_strokes_are_normalized_simplified_and_undoable() {
        let mut model = CanvasModel::new(Canvas::default()).unwrap();
        let id = model
            .add_pencil_stroke(
                &[
                    Point { x: 10.0, y: 20.0 },
                    Point { x: 11.0, y: 21.0 },
                    Point { x: 20.0, y: 30.0 },
                ],
                Some(CanvasStyle {
                    stroke_color: Some("#ff00aa".into()),
                    stroke_width: Some(2.0),
                    ..Default::default()
                }),
            )
            .unwrap();
        let node = model
            .canvas()
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap();
        assert_eq!(
            (node.x, node.y, node.width, node.height),
            (10.0, 20.0, 10.0, 10.0)
        );
        assert!(matches!(
            &node.kind,
            CanvasNodeKind::Pencil { coords, .. }
                if coords.first() == Some(&(0.0, 0.0))
                    && coords.last() == Some(&(10.0, 10.0))
        ));
        assert!(model.undo());
        assert!(model.canvas().nodes.is_empty());
    }

    #[test]
    fn fit_to_view_centers_content_without_changing_canvas_data() {
        let mut model = CanvasModel::new(Canvas {
            nodes: vec![CanvasNode::new(
                CanvasNodeKind::Shape {
                    shape: ShapeType::Rectangle,
                },
                100.0,
                50.0,
                200.0,
                100.0,
            )],
            ..Default::default()
        })
        .unwrap();
        let before = model.canvas().clone();
        model.fit_to_view(1000.0, 600.0, 50.0);
        assert_eq!(model.canvas(), &before);
        assert_eq!(model.viewport.scale, 4.0);
        assert_eq!(
            model
                .viewport
                .canvas_to_screen(Point { x: 100.0, y: 50.0 })
                .x,
            100.0
        );
    }

    #[test]
    fn rejects_unsafe_links_and_invalid_styles() {
        let mut link = CanvasNode::new(
            CanvasNodeKind::Link {
                url: "javascript:alert(1)".into(),
            },
            0.0,
            0.0,
            100.0,
            50.0,
        );
        assert!(
            Canvas {
                nodes: vec![link.clone()],
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        link.kind = CanvasNodeKind::Link {
            url: "https://example.com".into(),
        };
        link.style = Some(CanvasStyle {
            opacity: Some(2.0),
            ..Default::default()
        });
        assert!(
            Canvas {
                nodes: vec![link],
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn remote_reconciliation_merges_disjoint_entity_edits() {
        let first = CanvasNode::text("A", 0.0, 0.0);
        let second = CanvasNode::text("B", 100.0, 0.0);
        let base = Canvas {
            nodes: vec![first.clone(), second],
            ..Default::default()
        };
        let mut model = CanvasModel::new(base.clone()).unwrap();
        model.select_only(&first.id);
        model.update_selected_text("Local A".into()).unwrap();

        let mut remote = base;
        let CanvasNodeKind::Text { text, .. } = &mut remote.nodes[1].kind else {
            unreachable!()
        };
        *text = "Remote B".into();
        assert_eq!(
            model.reconcile_remote(&remote).unwrap(),
            CanvasReconcileOutcome::Merged
        );
        assert!(matches!(
            &model.canvas().nodes[0].kind,
            CanvasNodeKind::Text { text, .. } if text == "Local A"
        ));
        assert!(matches!(
            &model.canvas().nodes[1].kind,
            CanvasNodeKind::Text { text, .. } if text == "Remote B"
        ));
        assert!(model.dirty);
    }

    #[test]
    fn remote_reconciliation_merges_disjoint_fields_on_the_same_node() {
        let mut first = CanvasNode::text("A", 0.0, 0.0);
        first.style = Some(CanvasStyle::default());
        let base = Canvas {
            nodes: vec![first.clone()],
            ..Default::default()
        };
        let mut model = CanvasModel::new(base.clone()).unwrap();
        model.select_only(&first.id);
        assert!(model.move_selected(Point { x: 50.0, y: 25.0 }));
        model
            .update_selected_node(
                200.0,
                48.0,
                None,
                CanvasStyle {
                    fill_color: Some("#ff0000".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let mut remote = base;
        let CanvasNodeKind::Text { text, .. } = &mut remote.nodes[0].kind else {
            unreachable!()
        };
        *text = "Remote copy".into();
        remote.nodes[0].style = Some(CanvasStyle {
            stroke_color: Some("#0000ff".into()),
            ..Default::default()
        });

        assert_eq!(
            model.reconcile_remote(&remote).unwrap(),
            CanvasReconcileOutcome::Merged
        );
        let merged = &model.canvas().nodes[0];
        assert_eq!((merged.x, merged.y), (50.0, 25.0));
        assert!(matches!(
            &merged.kind,
            CanvasNodeKind::Text { text, .. } if text == "Remote copy"
        ));
        let style = merged.style.as_ref().unwrap();
        assert_eq!(style.fill_color.as_deref(), Some("#ff0000"));
        assert_eq!(style.stroke_color.as_deref(), Some("#0000ff"));
    }

    #[test]
    fn remote_reconciliation_merges_concurrent_group_membership_changes() {
        let mut first = CanvasNode::text("A", 0.0, 0.0);
        first.id = "first".into();
        first.group_id = Some("group".into());
        let mut second = CanvasNode::text("B", 100.0, 0.0);
        second.id = "second".into();
        second.group_id = Some("group".into());
        let mut third = CanvasNode::text("C", 200.0, 0.0);
        third.id = "third".into();
        third.group_id = Some("group".into());
        let base = Canvas {
            nodes: vec![first, second, third],
            groups: vec![CanvasGroup {
                id: "group".into(),
                child_nodes: vec!["first".into()],
                child_edges: Vec::new(),
                sort_order: 1.0,
                layer: 0.0,
                extra: Map::new(),
            }],
            ..Default::default()
        };
        let mut model = CanvasModel::new(base.clone()).unwrap();
        model.canvas.groups[0].child_nodes.push("second".into());
        model.dirty = true;

        let mut remote = base;
        remote.groups[0].child_nodes.push("third".into());
        assert_eq!(
            model.reconcile_remote(&remote).unwrap(),
            CanvasReconcileOutcome::Merged
        );
        let children = model.canvas().groups[0]
            .child_nodes
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        assert_eq!(children, HashSet::from(["first", "second", "third"]));
    }

    #[test]
    fn remote_reconciliation_reports_same_entity_conflicts_without_data_loss() {
        let first = CanvasNode::text("A", 0.0, 0.0);
        let base = Canvas {
            nodes: vec![first.clone()],
            ..Default::default()
        };
        let mut model = CanvasModel::new(base.clone()).unwrap();
        model.select_only(&first.id);
        model.update_selected_text("Local".into()).unwrap();
        let local = model.canvas().clone();

        let mut remote = base;
        let CanvasNodeKind::Text { text, .. } = &mut remote.nodes[0].kind else {
            unreachable!()
        };
        *text = "Remote".into();
        assert_eq!(
            model.reconcile_remote(&remote).unwrap(),
            CanvasReconcileOutcome::Conflict
        );
        assert_eq!(model.canvas(), &local);
        assert!(model.dirty);
    }

    #[test]
    fn svg_export_uses_connected_sides_markers_and_escaped_labels() {
        let mut first = CanvasNode::new(
            CanvasNodeKind::Text {
                text: "A & <B>".into(),
                follow_text_width: Some(true),
            },
            10.0,
            20.0,
            100.0,
            40.0,
        );
        first.id = "first".into();
        let mut second = CanvasNode::shape(ShapeType::Rectangle, 200.0, 20.0);
        second.id = "second".into();
        let canvas = Canvas {
            nodes: vec![first, second],
            edges: vec![CanvasEdge {
                id: "edge".into(),
                group_id: None,
                from: EdgeEnd::Connected {
                    node: "first".into(),
                    side: EdgeSide::Right,
                },
                to: EdgeEnd::Connected {
                    node: "second".into(),
                    side: EdgeSide::Left,
                },
                label: Some("A < B & C".into()),
                style: Some(CanvasStyle {
                    stroke_color: Some("#123456".into()),
                    from_end_style: Some(json!("circle-small")),
                    to_end_style: Some(json!(2)),
                    ..Default::default()
                }),
                sort_order: 0.0,
                layer: 0.0,
                extra: Map::new(),
            }],
            ..Default::default()
        };

        let svg = canvas.to_svg().unwrap();
        assert!(svg.contains("M 110 40 L 200 70"));
        assert!(svg.contains("marker-start=\"url(#macro-circle-small)\""));
        assert!(svg.contains("marker-end=\"url(#macro-arrow-filled)\""));
        assert!(svg.contains("A &amp; &lt;B&gt;"));
        assert!(svg.contains("A &lt; B &amp; C"));
    }

    #[test]
    fn free_connector_bounds_are_exported_without_nodes() {
        let canvas = Canvas {
            edges: vec![CanvasEdge {
                id: "edge".into(),
                group_id: None,
                from: EdgeEnd::Free { x: -10.0, y: 5.0 },
                to: EdgeEnd::Free { x: 30.0, y: 45.0 },
                label: None,
                style: None,
                sort_order: 0.0,
                layer: 0.0,
                extra: Map::new(),
            }],
            ..Default::default()
        };

        assert_eq!(
            canvas.content_bounds(),
            Some((Point { x: -10.0, y: 5.0 }, Point { x: 40.0, y: 40.0 }))
        );
        let svg = canvas.to_svg().unwrap();
        assert!(svg.contains("viewBox=\"-34 -19 88 88\""));
        assert!(svg.contains("M -10 5 L 30 45"));
    }
}
