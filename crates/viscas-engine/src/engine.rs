//! [`Engine`]: the scene, its evaluator, the console and the plot
//! pipeline behind one UI-independent API.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use viscas_core::eval::is_reserved;
use viscas_core::syntax::ParseError;
use viscas_plot::{
    CancelToken, Feature, FeatureKind, PlotOptions, Vec2, View, explicit_features,
    find_intersections,
};
use viscas_scene::{
    Builtins, Color, DocumentError, KindTag, NodeContent, NodeId, NodeKind, Scene, SceneError,
    SceneEvent, parse_definition_with,
};

use crate::console::ConsoleState;
use crate::eval::{NodeValue, SceneEvaluator};
use crate::plot::{CompiledPlot, PlotItem, PlotJob, PlotLayer, PlotShape, PlotStyle, ViewportId};
use crate::settings::EngineSettings;
use crate::worker::{PlotPool, PlotResult};

/// Errors of engine operations.
#[derive(Debug)]
pub enum EngineError {
    Io(std::io::Error),
    Document(DocumentError),
    Scene(SceneError),
    /// An expression-list line did not parse.
    Parse(Vec<ParseError>),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Io(e) => write!(f, "{e}"),
            EngineError::Document(e) => write!(f, "{e}"),
            EngineError::Scene(e) => write!(f, "{e}"),
            EngineError::Parse(errs) => match errs.first() {
                Some(e) => write!(f, "{}", e.message),
                None => f.write_str("parse error"),
            },
        }
    }
}

impl std::error::Error for EngineError {}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::Io(e)
    }
}

impl From<DocumentError> for EngineError {
    fn from(e: DocumentError) -> Self {
        EngineError::Document(e)
    }
}

impl From<SceneError> for EngineError {
    fn from(e: SceneError) -> Self {
        EngineError::Scene(e)
    }
}

/// Built-in color cycle used until the app injects its theme palette.
pub const DEFAULT_PALETTE: [Color; 8] = [
    Color::rgb(0x4c, 0x9a, 0xff),
    Color::rgb(0xff, 0x6b, 0x5b),
    Color::rgb(0x3d, 0xc9, 0x8a),
    Color::rgb(0xf5, 0xb7, 0x2b),
    Color::rgb(0xb3, 0x7c, 0xff),
    Color::rgb(0x2e, 0xc4, 0xd6),
    Color::rgb(0xff, 0x8a, 0xc8),
    Color::rgb(0xa0, 0xa8, 0xb8),
];

/// A root, extremum or intersection near a clicked point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureHit {
    pub node: NodeId,
    /// The other curve of an intersection.
    pub other: Option<NodeId>,
    pub feature: Feature,
    /// World distance to the query point.
    pub distance: f64,
}

struct ViewportState {
    view: Option<View>,
    generation: u64,
    cancel: CancelToken,
    pending: usize,
}

impl Default for ViewportState {
    fn default() -> Self {
        ViewportState {
            view: None,
            generation: 0,
            cancel: CancelToken::new(),
            pending: 0,
        }
    }
}

struct CacheEntry {
    view: View,
    revision: u64,
    layer: PlotLayer,
}

/// Does a node kind get a palette color when created?
fn colored_kind(kind: &NodeKind) -> bool {
    matches!(
        kind.tag(),
        KindTag::Plot | KindTag::Point | KindTag::List | KindTag::Function
    )
}

/// The scene builtin oracle backed by the evaluator's tables.
pub fn builtins_oracle() -> Arc<dyn Builtins> {
    Arc::new(|name: &str| is_reserved(name) || (name.starts_with("log_") && name.len() > 4))
}

/// Scene + evaluator + console + background plotting. Lives on the UI
/// thread; plotting runs on its worker pool.
///
/// Typical frame: mutate through [`Engine::scene_mut`] (or the helpers),
/// call [`Engine::update`], [`Engine::drain_events`] to refresh widgets,
/// [`Engine::request_plots`] when a view or the scene changed, and
/// [`Engine::poll_plots`] + [`Engine::layers`] on a timer/idle callback.
pub struct Engine {
    scene: Scene,
    eval: SceneEvaluator,
    interrupt: Arc<AtomicBool>,
    events: Vec<SceneEvent>,
    palette: Vec<Color>,
    palette_next: usize,
    auto_colors: HashMap<NodeId, Color>,
    pool: PlotPool,
    viewports: HashMap<ViewportId, ViewportState>,
    cache: HashMap<(NodeId, ViewportId), CacheEntry>,
    plot_options: PlotOptions,
    pub(crate) console: ConsoleState,
    path: Option<PathBuf>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("nodes", &self.scene.len())
            .field("settings", self.eval.settings())
            .field("workers", &self.pool.threads())
            .finish()
    }
}

impl Engine {
    /// Empty document, default settings, default worker count.
    pub fn new() -> Engine {
        Engine::with_workers(PlotPool::default_threads())
    }

    /// Empty document with `threads` plot workers.
    pub fn with_workers(threads: usize) -> Engine {
        let interrupt = Arc::new(AtomicBool::new(false));
        let settings = EngineSettings::default();
        let mut scene = Scene::new();
        scene.set_builtins(builtins_oracle());
        scene.clear_history();
        scene.drain_events();
        Engine {
            scene,
            eval: SceneEvaluator::new(settings, interrupt.clone()),
            interrupt,
            events: Vec::new(),
            palette: DEFAULT_PALETTE.to_vec(),
            palette_next: 0,
            auto_colors: HashMap::new(),
            pool: PlotPool::new(threads),
            viewports: HashMap::new(),
            cache: HashMap::new(),
            plot_options: PlotOptions::default(),
            console: ConsoleState::default(),
            path: None,
        }
    }

    // ------------------------------------------------------------------
    // Scene access and evaluation
    // ------------------------------------------------------------------

    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Mutable scene access; call [`Engine::update`] afterwards.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    pub fn evaluator(&self) -> &SceneEvaluator {
        &self.eval
    }

    /// Flag that interrupts long evaluations when set (from any thread).
    /// It is cleared at the start of every [`Engine::update`] and console
    /// evaluation.
    pub fn interrupt_flag(&self) -> Arc<AtomicBool> {
        self.interrupt.clone()
    }

    /// Evaluate every dirty node (inputs first), invalidate plot caches
    /// of changed nodes and queue the scene's events. Returns the
    /// processed nodes in evaluation order.
    pub fn update(&mut self) -> Vec<NodeId> {
        self.interrupt.store(false, Ordering::Relaxed);
        self.eval.prune(&self.scene);
        // A dirty node must not leak its previous value to dependents
        // evaluated before it is recomputed (or when it is blocked).
        let dirty = self.scene.dirty();
        self.eval.forget_many(&dirty);
        let done = self.scene.evaluate_dirty(&mut self.eval);
        let scene = &self.scene;
        let eval = &self.eval;
        self.cache
            .retain(|(n, _), e| scene.contains(*n) && e.revision == eval.revision(*n));
        self.auto_colors.retain(|id, _| scene.contains(*id));
        let mut missing: Vec<NodeId> = done
            .iter()
            .copied()
            .filter(|id| {
                self.eval.plot(*id).is_some()
                    && !self.auto_colors.contains_key(id)
                    && self.scene.get(*id).is_some_and(|n| n.style.color.is_none())
            })
            .collect();
        missing.sort_unstable();
        for id in missing {
            let c = self.next_color();
            self.auto_colors.insert(id, c);
        }
        self.events.extend(self.scene.drain_events());
        done
    }

    /// Scene events queued since the last call.
    pub fn drain_events(&mut self) -> Vec<SceneEvent> {
        self.events.extend(self.scene.drain_events());
        std::mem::take(&mut self.events)
    }

    /// Evaluated value of a node.
    pub fn value(&self, id: NodeId) -> Option<&NodeValue> {
        self.eval.value(id)
    }

    /// Compiled plot of a node.
    pub fn plot_of(&self, id: NodeId) -> Option<&Arc<CompiledPlot>> {
        self.eval.plot(id)
    }

    pub fn settings(&self) -> &EngineSettings {
        self.eval.settings()
    }

    /// Change evaluation settings; re-evaluates everything when they
    /// differ.
    pub fn set_settings(&mut self, settings: EngineSettings) {
        if settings == *self.eval.settings() {
            return;
        }
        self.eval.set_settings(settings);
        self.eval.reset();
        self.cache.clear();
        self.scene.mark_all_dirty();
        self.update();
    }

    /// Add a node (palette color assigned to plot-like kinds without one,
    /// in the same undo step) and evaluate.
    pub fn add_node(
        &mut self,
        parent: Option<NodeId>,
        mut content: NodeContent,
    ) -> Result<NodeId, EngineError> {
        if content.style.color.is_none() && colored_kind(&content.kind) {
            content.style.color = Some(self.next_color());
        }
        let id = self.scene.add(parent, content)?;
        self.update();
        Ok(id)
    }

    /// Parse an expression-list line (`a := 3`, `f(x) := x^2`, `y = x^2`)
    /// with the current definition operator and add it as a node.
    pub fn add_line(&mut self, parent: Option<NodeId>, line: &str) -> Result<NodeId, EngineError> {
        let content = parse_definition_with(line, self.settings().parse_options())
            .map_err(EngineError::Parse)?;
        self.add_node(parent, content)
    }

    pub fn undo(&mut self) -> bool {
        let r = self.scene.undo();
        self.update();
        r
    }

    pub fn redo(&mut self) -> bool {
        let r = self.scene.redo();
        self.update();
        r
    }

    // ------------------------------------------------------------------
    // Colors
    // ------------------------------------------------------------------

    /// Replace the color cycle (theme colors). Ignored when empty.
    pub fn set_palette(&mut self, palette: Vec<Color>) {
        if !palette.is_empty() {
            self.palette = palette;
            self.palette_next = 0;
        }
    }

    pub fn palette(&self) -> &[Color] {
        &self.palette
    }

    /// Next palette color (cycles).
    pub fn next_color(&mut self) -> Color {
        let c = self.palette[self.palette_next % self.palette.len()];
        self.palette_next = (self.palette_next + 1) % self.palette.len();
        c
    }

    /// The color a node is drawn with: its style color, else the
    /// automatic one assigned when it was first plotted.
    pub fn node_color(&self, id: NodeId) -> Option<Color> {
        let n = self.scene.get(id)?;
        n.style
            .color
            .or_else(|| self.auto_colors.get(&id).copied())
            .or_else(|| self.palette.first().copied())
    }

    // ------------------------------------------------------------------
    // Plotting
    // ------------------------------------------------------------------

    /// Sampling options of plot jobs (budgets, tolerances).
    pub fn plot_options(&self) -> &PlotOptions {
        &self.plot_options
    }

    pub fn set_plot_options(&mut self, options: PlotOptions) {
        self.plot_options = options;
        self.cache.clear();
    }

    /// Is the node and every ancestor visible?
    fn shown(&self, id: NodeId) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            match self.scene.get(c) {
                Some(n) if n.visible => cur = n.parent,
                _ => return false,
            }
        }
        true
    }

    /// Visible plot-like nodes with a compiled plot, in scene (draw) order.
    pub fn plot_items(&self) -> Vec<PlotItem> {
        self.scene
            .iter()
            .filter(|n| self.shown(n.id) && !self.scene.status(n.id).is_error())
            .filter_map(|n| {
                let plot = self.eval.plot(n.id)?.clone();
                Some(PlotItem {
                    node: n.id,
                    revision: self.eval.revision(n.id),
                    style: self.style_of(n.id),
                    plot,
                })
            })
            .collect()
    }

    fn style_of(&self, id: NodeId) -> PlotStyle {
        let auto = self.node_color(id).unwrap_or(DEFAULT_PALETTE[0]);
        match self.scene.get(id) {
            Some(n) => PlotStyle::from_style(&n.style, auto),
            None => PlotStyle::from_style(&Default::default(), auto),
        }
    }

    /// Build the job for `viewport` at `view` containing the nodes whose
    /// cached layer is missing or out of date, and make it the viewport's
    /// current job (cancelling the previous one). Use
    /// [`Engine::request_plots`] to run it on the pool.
    pub fn prepare_job(&mut self, viewport: ViewportId, view: View) -> PlotJob {
        let items: Vec<PlotItem> = self
            .plot_items()
            .into_iter()
            .filter(|it| {
                !self
                    .cache
                    .get(&(it.node, viewport))
                    .is_some_and(|e| e.view == view && e.revision == it.revision)
            })
            .collect();
        let state = self.viewports.entry(viewport).or_default();
        state.cancel.cancel();
        state.cancel = CancelToken::new();
        state.generation += 1;
        state.view = Some(view);
        state.pending = 0;
        let mut options = self.plot_options.clone();
        options.cancel = state.cancel.clone();
        PlotJob {
            viewport,
            generation: state.generation,
            view,
            items,
            cancel: state.cancel.clone(),
            options,
        }
    }

    /// Start (re)plotting `viewport` at `view` in the background. Cached
    /// layers are reused; a previous job of the viewport is cancelled.
    /// Returns the number of nodes being rendered.
    pub fn request_plots(&mut self, viewport: ViewportId, view: View) -> usize {
        let job = self.prepare_job(viewport, view);
        let n = job.items.len();
        if let Some(s) = self.viewports.get_mut(&viewport) {
            s.pending = n;
        }
        if n > 0 {
            self.pool.submit(job);
        }
        n
    }

    /// Render a job synchronously on the calling thread and store the
    /// results (headless export, tests).
    pub fn run_job_now(&mut self, job: &PlotJob) -> bool {
        match job.run() {
            Ok(layers) => {
                for (layer, item) in layers.into_iter().zip(&job.items) {
                    self.accept(PlotResult {
                        viewport: job.viewport,
                        generation: job.generation,
                        revision: item.revision,
                        view: job.view,
                        layer,
                    });
                }
                true
            }
            Err(_) => false,
        }
    }

    pub(crate) fn accept(&mut self, r: PlotResult) -> bool {
        let Some(state) = self.viewports.get_mut(&r.viewport) else {
            return false;
        };
        if r.generation != state.generation {
            return false;
        }
        state.pending = state.pending.saturating_sub(1);
        let node = r.layer.node_id;
        if !self.scene.contains(node) || r.revision != self.eval.revision(node) {
            return false;
        }
        self.cache.insert(
            (node, r.viewport),
            CacheEntry {
                view: r.view,
                revision: r.revision,
                layer: r.layer,
            },
        );
        true
    }

    /// Drain finished background results into the cache. Returns the
    /// viewports that received fresh layers (redraw them).
    pub fn poll_plots(&mut self) -> Vec<ViewportId> {
        let mut out = Vec::new();
        while let Some(r) = self.pool.try_recv() {
            let vp = r.viewport;
            if self.accept(r) && !out.contains(&vp) {
                out.push(vp);
            }
        }
        out
    }

    /// Block until every viewport's current job finished or `timeout`
    /// elapsed (tests, export). Returns `true` when all are done.
    pub fn wait_for_plots(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_plots();
            if self.viewports.values().all(|s| s.pending == 0) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            if let Some(r) = self.pool.recv_timeout(deadline - now) {
                self.accept(r);
            }
        }
    }

    /// Nodes still being rendered for the viewport's current job.
    pub fn pending_plots(&self, viewport: ViewportId) -> usize {
        self.viewports.get(&viewport).map_or(0, |s| s.pending)
    }

    /// The view of the viewport's latest request.
    pub fn viewport_view(&self, viewport: ViewportId) -> Option<View> {
        self.viewports.get(&viewport).and_then(|s| s.view)
    }

    /// Cancel and drop a viewport (editor closed).
    pub fn close_viewport(&mut self, viewport: ViewportId) {
        if let Some(s) = self.viewports.remove(&viewport) {
            s.cancel.cancel();
        }
        self.cache.retain(|(_, v), _| *v != viewport);
    }

    /// Best available layers of a viewport in draw order. Layers computed
    /// for an older view are returned with `stale = true` (still correct
    /// in world coordinates); styles always reflect the current node style.
    pub fn layers(&self, viewport: ViewportId) -> Vec<PlotLayer> {
        let view = self.viewport_view(viewport);
        self.plot_items()
            .into_iter()
            .filter_map(|it| {
                let e = self.cache.get(&(it.node, viewport))?;
                if e.revision != it.revision {
                    return None;
                }
                let mut layer = e.layer.clone();
                layer.style = it.style;
                layer.stale = Some(e.view) != view;
                Some(layer)
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Analysis
    // ------------------------------------------------------------------

    /// Roots, extrema, y-intercepts and pairwise intersections of the
    /// visible explicit curves `y = f(x)` within `radius` (world units) of
    /// `point`, nearest first. Uses the viewport's latest view; empty if
    /// it was never plotted.
    pub fn features_near(&self, viewport: ViewportId, point: Vec2, radius: f64) -> Vec<FeatureHit> {
        let Some(view) = self.viewport_view(viewport) else {
            return Vec::new();
        };
        let opts = PlotOptions {
            cancel: CancelToken::none(),
            ..self.plot_options.clone()
        };
        let curves: Vec<(NodeId, crate::plot::Fn1)> = self
            .plot_items()
            .into_iter()
            .filter_map(|it| match &it.plot.shape {
                PlotShape::ExplicitY(f) => Some((it.node, f.clone())),
                _ => None,
            })
            .collect();
        let mut hits = Vec::new();
        let mut push = |node, other, feature: Feature| {
            let distance = (feature.pos - point).length();
            if distance <= radius {
                hits.push(FeatureHit {
                    node,
                    other,
                    feature,
                    distance,
                });
            }
        };
        for (node, f) in &curves {
            if let Ok(fs) = explicit_features(&**f, &view, &opts) {
                for ft in fs.data {
                    push(*node, None, ft);
                }
            }
        }
        let b = view.bounds();
        let samples = view.width_px().ceil() as usize;
        for (i, (a, f)) in curves.iter().enumerate() {
            for (bn, g) in &curves[i + 1..] {
                if let Ok(pts) = find_intersections(&**f, &**g, b.x_min, b.x_max, samples, &opts) {
                    for p in pts.data {
                        let ft = Feature {
                            pos: p,
                            kind: FeatureKind::Intersection,
                        };
                        push(*a, Some(*bn), ft);
                    }
                }
            }
        }
        hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        hits
    }

    // ------------------------------------------------------------------
    // Documents
    // ------------------------------------------------------------------

    fn replace_scene(&mut self, mut scene: Scene) {
        scene.set_builtins(builtins_oracle());
        scene.clear_history();
        scene.drain_events();
        self.scene = scene;
        self.eval.reset();
        self.cache.clear();
        for s in self.viewports.values_mut() {
            s.cancel.cancel();
            s.pending = 0;
            s.generation += 1;
        }
        self.auto_colors.clear();
        self.palette_next = 0;
        self.console.forget_pin();
        self.events.push(SceneEvent::GraphChanged {
            touched: Vec::new(),
        });
        self.events.push(SceneEvent::HistoryChanged);
        self.update();
    }

    /// Start an empty document.
    pub fn new_document(&mut self) {
        self.path = None;
        self.replace_scene(Scene::new());
    }

    /// Load `.viscas` text (the app layout blob is kept with the scene).
    pub fn open_str(&mut self, text: &str) -> Result<(), DocumentError> {
        let scene = Scene::load_from_str(text)?;
        self.path = None;
        self.replace_scene(scene);
        Ok(())
    }

    /// Load a `.viscas` file.
    pub fn open_path(&mut self, path: impl AsRef<Path>) -> Result<(), EngineError> {
        let text = std::fs::read_to_string(path.as_ref())?;
        self.open_str(&text)?;
        self.path = Some(path.as_ref().to_path_buf());
        Ok(())
    }

    /// Serialize the document.
    pub fn save_string(&self) -> Result<String, DocumentError> {
        self.scene.save_to_string()
    }

    /// Write the document to `path` (remembered as the document path).
    pub fn save_path(&mut self, path: impl AsRef<Path>) -> Result<(), EngineError> {
        let text = self.save_string()?;
        std::fs::write(path.as_ref(), text)?;
        self.path = Some(path.as_ref().to_path_buf());
        Ok(())
    }

    /// Path of the last opened/saved file.
    pub fn document_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The app's opaque layout blob, saved with the document.
    pub fn layout(&self) -> Option<&serde_json::Value> {
        self.scene.layout()
    }

    pub fn set_layout(&mut self, layout: Option<serde_json::Value>) {
        self.scene.set_layout(layout);
    }
}
