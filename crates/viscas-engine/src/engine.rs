//! [`Engine`]: the scene, its evaluator, the console and the plot
//! pipeline behind one UI-independent API.

use std::collections::HashMap;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use viscas_core::eval::is_reserved;
use viscas_core::syntax::ParseError;
use viscas_plot::{
    CancelToken, Feature, FeatureKind, PlotOptions, Vec2, View, explicit_features,
    find_intersections,
};
use viscas_scene::{
    Builtins, Change, Color, DocumentError, KindTag, NodeContent, NodeId, NodeKind, Scene,
    SceneError, SceneEvent, parse_definition_with,
};

use crate::console::ConsoleState;
use crate::eval::{Defs, NodeValue, SceneEvaluator};
use crate::lines::line_parse_options;
use crate::plot::{
    CompiledPlot, Fn1, PlotItem, PlotJob, PlotLayer, PlotShape, PlotStyle, ViewportId,
};
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

/// At most this many curves take part in feature analysis (pairs grow
/// quadratically).
pub const MAX_FEATURE_CURVES: usize = 8;
/// Evaluation budget of one feature-analysis call.
const FEATURE_EVALS: usize = 20_000;

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

/// Result of [`Engine::request_features`].
#[derive(Clone, Debug, PartialEq)]
pub struct FeaturesResult {
    pub ticket: u64,
    pub viewport: ViewportId,
    pub hits: Vec<FeatureHit>,
}

struct ViewportState {
    view: Option<View>,
    /// Interrupt flag of the current jobs (also their cancel token).
    flag: Arc<AtomicBool>,
    /// Results of jobs older than this generation are discarded.
    epoch: u64,
    /// Node → revision being rendered by a current job.
    in_flight: HashMap<NodeId, u64>,
}

impl Default for ViewportState {
    fn default() -> Self {
        ViewportState {
            view: None,
            flag: Arc::new(AtomicBool::new(false)),
            epoch: 0,
            in_flight: HashMap::new(),
        }
    }
}

struct CacheEntry {
    view: View,
    revision: u64,
    layer: PlotLayer,
    /// `false` for error layers: shown, but re-rendered on the next request.
    complete: bool,
}

impl CacheEntry {
    fn fresh(&self, view: View, revision: u64) -> bool {
        self.complete && self.view == view && self.revision == revision
    }
}

/// Does a node kind get a palette color?
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
/// [`Engine::request_plots`] for the viewports in
/// [`Engine::needs_replot`] (or whose view changed), and
/// [`Engine::poll_plots`] + [`Engine::layers`] on a timer/idle callback.
pub struct Engine {
    scene: Scene,
    eval: SceneEvaluator,
    interrupt: Arc<AtomicBool>,
    events: Vec<SceneEvent>,
    palette: Vec<Color>,
    palette_next: usize,
    /// Automatic colors of nodes without a style color (never saved).
    auto_colors: HashMap<NodeId, Color>,
    pool: PlotPool,
    plot_tx: Sender<PlotResult>,
    plot_rx: Receiver<PlotResult>,
    feat_tx: Sender<FeaturesResult>,
    feat_rx: Receiver<FeaturesResult>,
    feat_flag: Arc<AtomicBool>,
    feat_ticket: u64,
    viewports: HashMap<ViewportId, ViewportState>,
    next_viewport: u64,
    node_viewports: HashMap<NodeId, ViewportId>,
    next_generation: u64,
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

impl Drop for Engine {
    fn drop(&mut self) {
        // Interrupt running samples so the pool joins promptly.
        for s in self.viewports.values() {
            s.flag.store(true, Ordering::Relaxed);
        }
        self.feat_flag.store(true, Ordering::Relaxed);
        self.interrupt.store(true, Ordering::Relaxed);
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
        let (plot_tx, plot_rx) = mpsc::channel();
        let (feat_tx, feat_rx) = mpsc::channel();
        Engine {
            scene,
            eval: SceneEvaluator::new(settings, interrupt.clone()),
            interrupt,
            events: Vec::new(),
            palette: DEFAULT_PALETTE.to_vec(),
            palette_next: 0,
            auto_colors: HashMap::new(),
            pool: PlotPool::new(threads),
            plot_tx,
            plot_rx,
            feat_tx,
            feat_rx,
            feat_flag: Arc::new(AtomicBool::new(false)),
            feat_ticket: 0,
            viewports: HashMap::new(),
            next_viewport: 1,
            node_viewports: HashMap::new(),
            next_generation: 1,
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

    /// Snapshot of the scene definitions (console, analysis).
    pub(crate) fn defs_snapshot(&mut self) -> Defs {
        self.eval.snapshot()
    }

    /// Flag that interrupts long scene/console evaluations when set (from
    /// any thread). Cleared at the start of every [`Engine::update`] and
    /// console evaluation. Plot jobs use their own flags.
    pub fn interrupt_flag(&self) -> Arc<AtomicBool> {
        self.interrupt.clone()
    }

    /// Evaluate every dirty node (inputs first), compile plots and queue
    /// the scene's events. Property edits of plot nodes (`t_min`, …) make
    /// them dirty. Returns the processed nodes in evaluation order.
    pub fn update(&mut self) -> Vec<NodeId> {
        self.interrupt.store(false, Ordering::Relaxed);
        let events = self.scene.drain_events();
        for e in &events {
            if let SceneEvent::Changed {
                id,
                what: Change::Props,
            } = e
                && matches!(
                    self.scene.get(*id).map(|n| &n.kind),
                    Some(NodeKind::Plot { .. })
                )
            {
                self.scene.mark_dirty(*id);
            }
        }
        self.events.extend(events);
        self.eval.prune(&self.scene);
        // A dirty node must not leak its previous value to dependents
        // evaluated before it is recomputed (or when it is blocked).
        let dirty = self.scene.dirty();
        self.eval.forget_many(&dirty);
        let done = self.scene.evaluate_dirty(&mut self.eval);
        self.eval.finish_pass(&mut self.scene);
        let scene = &self.scene;
        self.cache.retain(|(n, _), _| scene.contains(*n));
        self.auto_colors.retain(|id, _| scene.contains(*id));
        self.node_viewports.retain(|id, _| scene.contains(*id));
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
    ///
    /// A change of [`EngineSettings::approx`] alone only switches the
    /// display text (node values keep both forms), so nothing is
    /// re-evaluated.
    pub fn set_settings(&mut self, settings: EngineSettings) {
        let current = *self.eval.settings();
        if settings == current {
            return;
        }
        let display_only = EngineSettings {
            approx: current.approx,
            ..settings
        } == current;
        if display_only {
            self.eval.set_settings(settings);
            // Swap the stored display text of every valued node (emits
            // status events so editors refresh).
            let ids = self.scene.dfs_ids();
            for id in ids {
                let Some(text) = self
                    .eval
                    .value(id)
                    .map(|v| v.display(settings.approx).to_string())
                else {
                    continue;
                };
                if let Some(r) = self.scene.result(id) {
                    let r = viscas_scene::NodeResult {
                        status: r.status.clone(),
                        display: Some(text),
                    };
                    self.scene.set_result(id, r);
                }
            }
            self.events.extend(self.scene.drain_events());
            return;
        }
        self.eval.set_settings(settings);
        self.eval.reset();
        self.scene.mark_all_dirty();
        self.update();
    }

    /// Add a node and evaluate. Plot-like nodes without a style color get
    /// an automatic palette color (kept out of the saved style).
    pub fn add_node(
        &mut self,
        parent: Option<NodeId>,
        content: NodeContent,
    ) -> Result<NodeId, EngineError> {
        let content = self.with_auto_name(content);
        let id = self.scene.add(parent, content)?;
        self.assign_auto_color(id);
        self.update();
        Ok(id)
    }

    /// Gives an unnamed drawable node its Godot-style automatic name
    /// (`Plot1`, `Circle1`, `Point1`, ...; see [`viscas_scene::auto_name`]).
    pub(crate) fn with_auto_name(&self, mut content: NodeContent) -> NodeContent {
        if content.name.as_deref().is_none_or(|n| n.trim().is_empty()) {
            content.name = self.scene.next_auto_name(&content.kind);
        }
        content
    }

    pub(crate) fn assign_auto_color(&mut self, id: NodeId) {
        let wants = self
            .scene
            .get(id)
            .is_some_and(|n| n.style.color.is_none() && colored_kind(&n.kind));
        if wants && !self.auto_colors.contains_key(&id) {
            let c = self.next_color();
            self.auto_colors.insert(id, c);
        }
    }

    /// Parse an expression-list line (`a := 3`, `f(x) := x^2`, `y = x^2`)
    /// with the current definition operator and add it as a node.
    pub fn add_line(&mut self, parent: Option<NodeId>, line: &str) -> Result<NodeId, EngineError> {
        let content = self.line_content(line).map_err(EngineError::Parse)?;
        self.add_node(parent, content)
    }

    /// Node content of an expression-list line (see [`crate::lines`]).
    pub fn line_content(&self, line: &str) -> Result<NodeContent, Vec<ParseError>> {
        parse_definition_with(line, line_parse_options(line, self.settings()))
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

    /// Replace the color cycle (theme colors) and reassign automatic
    /// colors from it. Ignored when empty.
    pub fn set_palette(&mut self, palette: Vec<Color>) {
        if palette.is_empty() {
            return;
        }
        self.palette = palette;
        self.palette_next = 0;
        let mut ids: Vec<NodeId> = self.auto_colors.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let c = self.next_color();
            self.auto_colors.insert(id, c);
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

    /// The color a node is drawn with: its style color, else its
    /// automatic color.
    pub fn node_color(&self, id: NodeId) -> Option<Color> {
        let n = self.scene.get(id)?;
        n.style
            .color
            .or_else(|| self.auto_colors.get(&id).copied())
            .or_else(|| self.palette.first().copied())
    }

    // ------------------------------------------------------------------
    // Viewports and plotting
    // ------------------------------------------------------------------

    /// Allocate a viewport (one per graph editor).
    pub fn create_viewport(&mut self) -> ViewportId {
        let id = ViewportId(self.next_viewport);
        self.next_viewport += 1;
        self.viewports.insert(id, ViewportState::default());
        id
    }

    /// The viewport of a scene Viewport node (allocated on first use).
    pub fn viewport_for_node(&mut self, node: NodeId) -> ViewportId {
        if let Some(v) = self.node_viewports.get(&node)
            && self.viewports.contains_key(v)
        {
            return *v;
        }
        let v = self.create_viewport();
        self.node_viewports.insert(node, v);
        v
    }

    /// The viewport mapped to a Viewport node, if any.
    pub fn node_viewport(&self, node: NodeId) -> Option<ViewportId> {
        self.node_viewports.get(&node).copied()
    }

    /// Sampling options of plot jobs (budgets, tolerances).
    pub fn plot_options(&self) -> &PlotOptions {
        &self.plot_options
    }

    /// Change sampling options: cancels running jobs and drops caches.
    pub fn set_plot_options(&mut self, options: PlotOptions) {
        self.plot_options = options;
        self.cancel_all();
        self.cache.clear();
    }

    fn cancel_all(&mut self) {
        let epoch = self.next_generation;
        for s in self.viewports.values_mut() {
            s.flag.store(true, Ordering::Relaxed);
            s.flag = Arc::new(AtomicBool::new(false));
            s.in_flight.clear();
            s.epoch = epoch;
        }
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

    /// Visible plot-like nodes with a compiled plot, in scene order.
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

    /// Build the job for `viewport` at `view` with the nodes whose cached
    /// layer is missing, out of date or failed. With an unchanged view,
    /// nodes already being rendered are skipped and nothing is cancelled
    /// (an empty job means "nothing to do"); a new view cancels the
    /// viewport's running jobs. Use [`Engine::request_plots`] to run it on
    /// the pool. Unknown viewports yield an empty job.
    pub fn prepare_job(&mut self, viewport: ViewportId, view: View) -> PlotJob {
        let items = self.plot_items();
        let options = self.plot_options.clone();
        let generation = self.next_generation;
        let cache = &self.cache;
        let Some(state) = self.viewports.get_mut(&viewport) else {
            let cancel = CancelToken::new();
            cancel.cancel();
            return PlotJob {
                viewport,
                generation: 0,
                view,
                items: Vec::new(),
                cancel,
                options,
            };
        };
        let mut needed: Vec<PlotItem> = items
            .into_iter()
            .filter(|it| {
                !cache
                    .get(&(it.node, viewport))
                    .is_some_and(|e| e.fresh(view, it.revision))
            })
            .collect();
        let same_view = state.view == Some(view);
        if same_view {
            needed.retain(|it| state.in_flight.get(&it.node) != Some(&it.revision));
        } else {
            state.flag.store(true, Ordering::Relaxed);
            state.flag = Arc::new(AtomicBool::new(false));
            state.in_flight.clear();
            state.epoch = generation;
            state.view = Some(view);
        }
        let cancel = CancelToken::from(state.flag.clone());
        let mut options = options;
        options.cancel = cancel.clone();
        if needed.is_empty() {
            return PlotJob {
                viewport,
                generation: 0,
                view,
                items: needed,
                cancel,
                options,
            };
        }
        self.next_generation += 1;
        for it in &mut needed {
            state.in_flight.insert(it.node, it.revision);
            it.plot = it.plot.for_job(&state.flag);
        }
        PlotJob {
            viewport,
            generation,
            view,
            items: needed,
            cancel,
            options,
        }
    }

    /// Start (re)plotting `viewport` at `view` in the background (see
    /// [`Engine::prepare_job`]). Returns the number of nodes queued.
    pub fn request_plots(&mut self, viewport: ViewportId, view: View) -> usize {
        let job = Arc::new(self.prepare_job(viewport, view));
        let n = job.items.len();
        for i in 0..n {
            let job = job.clone();
            let tx = self.plot_tx.clone();
            self.pool.spawn(job.cancel.clone(), move || {
                if let Ok(layer) = job.run_item(i) {
                    let _ = tx.send(PlotResult {
                        viewport: job.viewport,
                        generation: job.generation,
                        revision: job.items[i].revision,
                        view: job.view,
                        layer,
                    });
                }
            });
        }
        n
    }

    /// Render a job synchronously on the calling thread and store the
    /// results (headless export, tests). `false` if it was cancelled.
    pub fn run_job_now(&mut self, job: &PlotJob) -> bool {
        for (i, item) in job.items.iter().enumerate() {
            match job.run_item(i) {
                Ok(layer) => {
                    self.accept(PlotResult {
                        viewport: job.viewport,
                        generation: job.generation,
                        revision: item.revision,
                        view: job.view,
                        layer,
                    });
                }
                Err(_) => return false,
            }
        }
        !job.cancel.is_cancelled()
    }

    pub(crate) fn accept(&mut self, r: PlotResult) -> bool {
        let Some(state) = self.viewports.get_mut(&r.viewport) else {
            return false;
        };
        if r.generation < state.epoch {
            return false;
        }
        let node = r.layer.node_id;
        if state.in_flight.get(&node) == Some(&r.revision) {
            state.in_flight.remove(&node);
        }
        if !self.scene.contains(node) || r.revision != self.eval.revision(node) {
            return false;
        }
        let complete = r.layer.error.is_none();
        self.cache.insert(
            (node, r.viewport),
            CacheEntry {
                view: r.view,
                revision: r.revision,
                layer: r.layer,
                complete,
            },
        );
        true
    }

    /// Drain finished background results into the cache. Returns the
    /// viewports that received fresh layers (redraw them).
    pub fn poll_plots(&mut self) -> Vec<ViewportId> {
        let mut out = Vec::new();
        while let Ok(r) = self.plot_rx.try_recv() {
            let vp = r.viewport;
            if self.accept(r) && !out.contains(&vp) {
                out.push(vp);
            }
        }
        out
    }

    /// Block until every viewport's current jobs finished or `timeout`
    /// elapsed (tests, export). Returns `true` when all are done.
    pub fn wait_for_plots(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_plots();
            if self.viewports.values().all(|s| s.in_flight.is_empty()) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            if let Ok(r) = self.plot_rx.recv_timeout(deadline - now) {
                self.accept(r);
            }
        }
    }

    /// Whether background plot work is queued or running for any
    /// viewport (keep polling while `true`).
    pub fn has_pending_work(&self) -> bool {
        self.pool.queued() > 0 || self.viewports.values().any(|s| !s.in_flight.is_empty())
    }

    /// Background tasks waiting for a worker.
    pub fn queued_tasks(&self) -> usize {
        self.pool.queued()
    }

    /// Nodes still being rendered for the viewport.
    pub fn pending_plots(&self, viewport: ViewportId) -> usize {
        self.viewports
            .get(&viewport)
            .map_or(0, |s| s.in_flight.len())
    }

    /// Viewports with a view whose layers are missing or out of date and
    /// not being rendered: call [`Engine::request_plots`] for them (e.g.
    /// after [`Engine::update`]).
    ///
    /// A failed (error) layer computed for the viewport's current view and
    /// the node's current revision does *not* count as missing here, so a
    /// persistent failure does not cause a replot loop; it is retried by
    /// the next explicit [`Engine::request_plots`] (new view or new
    /// revision).
    pub fn needs_replot(&self) -> Vec<ViewportId> {
        if self.viewports.values().all(|s| s.view.is_none()) {
            return Vec::new();
        }
        let items = self.plot_items();
        let mut out: Vec<ViewportId> = self
            .viewports
            .iter()
            .filter_map(|(vp, s)| {
                let view = s.view?;
                items
                    .iter()
                    .any(|it| {
                        !self
                            .cache
                            .get(&(it.node, *vp))
                            .is_some_and(|e| e.view == view && e.revision == it.revision)
                            && s.in_flight.get(&it.node) != Some(&it.revision)
                    })
                    .then_some(*vp)
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// The view of the viewport's latest request.
    pub fn viewport_view(&self, viewport: ViewportId) -> Option<View> {
        self.viewports.get(&viewport).and_then(|s| s.view)
    }

    /// Cancel and drop a viewport (editor closed).
    pub fn close_viewport(&mut self, viewport: ViewportId) {
        if let Some(s) = self.viewports.remove(&viewport) {
            s.flag.store(true, Ordering::Relaxed);
        }
        self.cache.retain(|(_, v), _| *v != viewport);
        self.node_viewports.retain(|_, v| *v != viewport);
    }

    /// Best available layers of a viewport: fills, then curves, then
    /// points (scene order within each group). Layers computed for an
    /// older view or an older revision of their node come back with
    /// `stale = true` until the fresh result arrives; styles always
    /// reflect the current node style. Cloning a layer shares its
    /// geometry.
    pub fn layers(&self, viewport: ViewportId) -> Vec<PlotLayer> {
        let view = self.viewport_view(viewport);
        let mut out: Vec<PlotLayer> = self
            .plot_items()
            .into_iter()
            .filter_map(|it| {
                let e = self.cache.get(&(it.node, viewport))?;
                let mut layer = e.layer.clone();
                layer.style = it.style;
                layer.stale = Some(e.view) != view || e.revision != it.revision;
                Some(layer)
            })
            .collect();
        out.sort_by_key(|l| l.kind.draw_rank());
        out
    }

    // ------------------------------------------------------------------
    // Analysis
    // ------------------------------------------------------------------

    fn feature_curves(&self, flag: Option<&Arc<AtomicBool>>) -> Vec<(NodeId, Fn1)> {
        self.plot_items()
            .into_iter()
            .filter_map(|it| {
                let plot = match flag {
                    Some(f) => it.plot.for_job(f),
                    None => it.plot,
                };
                match &plot.shape {
                    PlotShape::ExplicitY(f) => Some((it.node, f.clone())),
                    _ => None,
                }
            })
            .take(MAX_FEATURE_CURVES)
            .collect()
    }

    fn feature_options(&self, cancel: CancelToken) -> PlotOptions {
        PlotOptions {
            cancel,
            max_evals: FEATURE_EVALS,
            ..self.plot_options.clone()
        }
    }

    /// Roots, extrema, y-intercepts and pairwise intersections of the
    /// visible explicit curves `y = f(x)` within `radius` (world units) of
    /// `point`, nearest first, using the viewport's latest view. Bounded
    /// work (at most [`MAX_FEATURE_CURVES`] curves, a fixed evaluation
    /// budget per search); prefer [`Engine::request_features`] on the UI
    /// thread.
    pub fn features_near(&self, viewport: ViewportId, point: Vec2, radius: f64) -> Vec<FeatureHit> {
        let Some(view) = self.viewport_view(viewport) else {
            return Vec::new();
        };
        let opts = self.feature_options(CancelToken::none());
        compute_features(&self.feature_curves(None), view, point, radius, &opts)
    }

    /// [`Engine::features_near`] on the worker pool. A newer request
    /// cancels the previous one. Returns the ticket of the result
    /// delivered by [`Engine::poll_features`], or `None` when the viewport
    /// was never plotted.
    pub fn request_features(
        &mut self,
        viewport: ViewportId,
        point: Vec2,
        radius: f64,
    ) -> Option<u64> {
        let view = self.viewport_view(viewport)?;
        self.feat_flag.store(true, Ordering::Relaxed);
        self.feat_flag = Arc::new(AtomicBool::new(false));
        self.feat_ticket += 1;
        let ticket = self.feat_ticket;
        let cancel = CancelToken::from(self.feat_flag.clone());
        let curves = self.feature_curves(Some(&self.feat_flag));
        let opts = self.feature_options(cancel.clone());
        let tx = self.feat_tx.clone();
        self.pool.spawn(cancel.clone(), move || {
            let hits = compute_features(&curves, view, point, radius, &opts);
            if !cancel.is_cancelled() {
                let _ = tx.send(FeaturesResult {
                    ticket,
                    viewport,
                    hits,
                });
            }
        });
        Some(ticket)
    }

    /// Finished feature requests (only the latest ticket is kept).
    pub fn poll_features(&mut self) -> Vec<FeaturesResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.feat_rx.try_recv() {
            if r.ticket == self.feat_ticket {
                out.push(r);
            }
        }
        out
    }

    /// Wait up to `timeout` for the latest feature request.
    pub fn wait_for_features(&mut self, timeout: Duration) -> Option<FeaturesResult> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            match self.feat_rx.recv_timeout(deadline - now) {
                Ok(r) if r.ticket == self.feat_ticket => return Some(r),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
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
        self.cancel_all();
        self.cache.clear();
        self.node_viewports.clear();
        self.auto_colors.clear();
        self.palette_next = 0;
        self.console.forget_pin();
        // Events of the previous document must not leak into the new one.
        self.events.clear();
        self.events.push(SceneEvent::GraphChanged {
            touched: Vec::new(),
        });
        self.events.push(SceneEvent::HistoryChanged);
        let mut ids = self.scene.dfs_ids();
        ids.sort_unstable();
        for id in ids {
            self.assign_auto_color(id);
        }
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

    /// Write the document to `path` atomically (temporary file in the same
    /// directory, fsync, rename); remembered as the document path.
    pub fn save_path(&mut self, path: impl AsRef<Path>) -> Result<(), EngineError> {
        let text = self.save_string()?;
        atomic_write(path.as_ref(), text.as_bytes())?;
        self.path = Some(path.as_ref().to_path_buf());
        Ok(())
    }

    /// Read and parse a `.viscas` file without touching an engine (any
    /// thread); install the result with [`Engine::install_document`].
    pub fn load_file(path: &Path) -> Result<Scene, EngineError> {
        let text = std::fs::read_to_string(path)?;
        Ok(Scene::load_from_str(&text)?)
    }

    /// Replace the document with a scene from [`Engine::load_file`].
    pub fn install_document(&mut self, scene: Scene, path: Option<PathBuf>) {
        self.path = None;
        self.replace_scene(scene);
        self.path = path;
    }

    /// A `Send` closure that serializes the current document and writes
    /// it atomically to `path` (run it on any thread), then call
    /// [`Engine::set_document_path`] on success.
    pub fn save_task(
        &self,
        path: PathBuf,
    ) -> impl FnOnce() -> Result<(), EngineError> + Send + use<> {
        let doc = self.scene.to_document();
        move || {
            let text = viscas_scene::save_document(&doc)?;
            atomic_write(&path, text.as_bytes())?;
            Ok(())
        }
    }

    /// Remember `path` as the document path (after a background save).
    pub fn set_document_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
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

/// Feature search over explicit curves (any thread).
fn compute_features(
    curves: &[(NodeId, Fn1)],
    view: View,
    point: Vec2,
    radius: f64,
    opts: &PlotOptions,
) -> Vec<FeatureHit> {
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
    for (node, f) in curves {
        if let Ok(fs) = explicit_features(&**f, &view, opts) {
            for ft in fs.data {
                push(*node, None, ft);
            }
        }
    }
    let b = view.bounds();
    let samples = view.width_px().ceil() as usize;
    for (i, (a, f)) in curves.iter().enumerate() {
        for (bn, g) in &curves[i + 1..] {
            if let Ok(pts) = find_intersections(&**f, &**g, b.x_min, b.x_max, samples, opts) {
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

/// Write `bytes` to `path` via a synced temporary file in the same
/// directory and a rename, so a crash never leaves a truncated document.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("the document path has no file name"))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}
