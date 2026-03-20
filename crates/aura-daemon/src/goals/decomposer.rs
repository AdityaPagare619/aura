//! Goal decomposition engine — breaking high-level goals into executable sub-goals.
//!
//! Implements a Hierarchical Task Network (HTN) style decomposition:
//! 1. **Template matching**: known patterns (e.g. "send message" → open app, find contact, type,
//!    send)
//! 2. **ETG-guided**: if an ETG path exists, convert it directly into steps
//! 3. **LLM-assisted**: if no template matches, flag for neocortex decomposition
//!
//! Sub-goals form a DAG (not just linear), allowing parallel execution of
//! independent sub-tasks.

#[allow(unused_imports)]
// GoalStep/StepStatus re-imported in inner scopes; this is the canonical top-level import
use aura_types::goals::{Goal, GoalSource, GoalStatus, GoalStep, StepStatus};
use aura_types::{actions::ActionType, errors::GoalError};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use super::{BoundedMap, BoundedVec};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum decomposition depth to prevent infinite recursion.
const DEFAULT_MAX_DEPTH: usize = 5;

/// Maximum number of templates in the decomposer.
const MAX_TEMPLATES: usize = 128;

/// Maximum sub-goals per decomposition.
const MAX_SUB_GOALS: usize = 16;

/// Maximum steps per sub-goal.
const _MAX_STEPS_PER_GOAL: usize = 32;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Strategy selected for decomposing a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecompositionStrategy {
    /// Matched a known template.
    TemplateBased,
    /// ETG has a known execution path.
    EtgGuided,
    /// No template or ETG — needs LLM assistance.
    LlmAssisted,
    /// Goal is already atomic — no decomposition needed.
    Atomic,
}

/// A template for decomposing a class of goals.
///
/// Templates are matched against goal descriptions using keyword patterns.
/// When matched, they produce a deterministic set of sub-goal steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionTemplate {
    /// Unique template identifier.
    pub id: String,
    /// Keywords that trigger this template (any match activates it).
    pub trigger_keywords: Vec<String>,
    /// Required app package (if any).
    pub required_app: Option<String>,
    /// Ordered list of step templates.
    pub steps: Vec<StepTemplate>,
    /// Dependencies between steps as (from_index, to_index) pairs.
    /// This forms a DAG — step `to` cannot start until step `from` completes.
    pub dependencies: Vec<(usize, usize)>,
    /// Historical success rate for this template.
    pub success_rate: f32,
}

/// A single step within a decomposition template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTemplate {
    /// Human-readable description with placeholders (e.g. "Open {app}").
    pub description: String,
    /// The action type this step maps to (if known).
    pub action: Option<ActionType>,
    /// Maximum attempts before failure.
    pub max_attempts: u8,
}

/// A sub-goal produced by decomposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGoal {
    /// The goal object.
    pub goal: Goal,
    /// Indices of sub-goals this depends on (DAG edges).
    pub depends_on: Vec<usize>,
    /// Index in the decomposition output.
    pub index: usize,
}

/// Result of decomposing a goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionResult {
    /// The original parent goal ID.
    pub parent_goal_id: u64,
    /// Strategy used for this decomposition.
    pub strategy: DecompositionStrategy,
    /// Ordered sub-goals (respecting dependency DAG).
    pub sub_goals: Vec<SubGoal>,
    /// Confidence in this decomposition (0.0–1.0).
    pub confidence: f32,
    /// Estimated total duration in milliseconds.
    pub estimated_duration_ms: u32,
}

/// The goal decomposer — breaks high-level goals into executable sub-goal DAGs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalDecomposer {
    /// Known decomposition templates.
    templates: BoundedVec<DecompositionTemplate, MAX_TEMPLATES>,
    /// Maximum recursion depth for nested decomposition.
    max_depth: usize,
    /// Counter for generating unique sub-goal IDs.
    next_sub_goal_id: u64,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl GoalDecomposer {
    /// Create a new decomposer with default settings.
    pub fn new() -> Self {
        let mut decomposer = Self {
            templates: BoundedVec::new(),
            max_depth: DEFAULT_MAX_DEPTH,
            next_sub_goal_id: 10_000, // Start sub-goal IDs high to avoid collision.
        };
        decomposer.register_builtin_templates();
        decomposer
    }

    /// Create a decomposer with a custom max depth.
    pub fn with_max_depth(max_depth: usize) -> Self {
        let mut d = Self::new();
        d.max_depth = max_depth;
        d
    }

    /// Register a custom decomposition template.
    #[instrument(skip(self), fields(template_id = %template.id))]
    pub fn register_template(&mut self, template: DecompositionTemplate) -> Result<(), GoalError> {
        self.templates
            .try_push(template)
            .map_err(|_| GoalError::CapacityExceeded { max: MAX_TEMPLATES })
    }

    /// Decompose a goal into sub-goals.
    ///
    /// Tries strategies in order: template match → ETG lookup → LLM flag.
    /// Returns the decomposition result with a DAG of sub-goals.
    #[instrument(skip(self), fields(goal_id = goal.id, description = %goal.description))]
    pub fn decompose(
        &mut self,
        goal: &Goal,
        depth: usize,
    ) -> Result<DecompositionResult, GoalError> {
        if depth > self.max_depth {
            return Err(GoalError::DecompositionFailed(format!(
                "max depth {} exceeded at depth {}",
                self.max_depth, depth
            )));
        }

        // Strategy 1: Try template matching.
        if let Some(result) = self.try_template_decomposition(goal) {
            tracing::info!(
                goal_id = goal.id,
                strategy = "template",
                sub_goals = result.sub_goals.len(),
                "goal decomposed via template"
            );
            return Ok(result);
        }

        // Strategy 2: Check if goal has existing steps (ETG-guided or pre-planned).
        if !goal.steps.is_empty() {
            let result = self.from_existing_steps(goal);
            tracing::info!(
                goal_id = goal.id,
                strategy = "etg",
                sub_goals = result.sub_goals.len(),
                "goal decomposed from existing steps"
            );
            return Ok(result);
        }

        // Strategy 3: Flag for LLM assistance — produce a single sub-goal
        // that requires neocortex planning.
        let result = self.flag_for_llm(goal);
        tracing::info!(
            goal_id = goal.id,
            strategy = "llm",
            "goal flagged for LLM decomposition"
        );
        Ok(result)
    }

    /// Check if a goal is atomic (doesn't need decomposition).
    #[must_use]
    pub fn is_atomic(&self, goal: &Goal) -> bool {
        // A goal with 0 or 1 steps is atomic.
        goal.steps.len() <= 1
    }

    /// Number of registered templates.
    #[must_use]
    pub fn template_count(&self) -> usize {
        self.templates.len()
    }

    /// Validate a decomposition result for dependency cycles.
    #[instrument(skip(self, result))]
    pub fn validate_dag(&self, result: &DecompositionResult) -> Result<(), GoalError> {
        let n = result.sub_goals.len();
        if n == 0 {
            return Ok(());
        }

        // Build adjacency list and do topological sort (Kahn's algorithm).
        let mut in_degree = vec![0u32; n];
        let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

        for sg in &result.sub_goals {
            for &dep in &sg.depends_on {
                if dep >= n {
                    return Err(GoalError::DependencyCycle(format!(
                        "dependency index {} out of range (max {})",
                        dep,
                        n - 1
                    )));
                }
                adj[dep].push(sg.index);
                in_degree[sg.index] = in_degree[sg.index].saturating_add(1);
            }
        }

        // Kahn's: start with zero in-degree nodes.
        let mut queue: Vec<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &d)| d == 0)
            .map(|(i, _)| i)
            .collect();

        let mut visited = 0usize;
        while let Some(node) = queue.pop() {
            visited += 1;
            for &next in &adj[node] {
                in_degree[next] = in_degree[next].saturating_sub(1);
                if in_degree[next] == 0 {
                    queue.push(next);
                }
            }
        }

        if visited != n {
            return Err(GoalError::DependencyCycle(format!(
                "cycle detected: visited {} of {} nodes",
                visited, n
            )));
        }

        Ok(())
    }

    /// Get the topological execution order for sub-goals.
    ///
    /// Returns indices in an order where all dependencies are satisfied
    /// before a sub-goal is executed.
    pub fn topological_order(&self, result: &DecompositionResult) -> Result<Vec<usize>, GoalError> {
        let n = result.sub_goals.len();
        if n == 0 {
            return Ok(vec![]);
        }

        let mut in_degree = vec![0u32; n];
        let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

        for sg in &result.sub_goals {
            for &dep in &sg.depends_on {
                if dep >= n {
                    return Err(GoalError::DependencyCycle(format!(
                        "dependency index {} out of range",
                        dep
                    )));
                }
                adj[dep].push(sg.index);
                in_degree[sg.index] = in_degree[sg.index].saturating_add(1);
            }
        }

        let mut queue: Vec<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &d)| d == 0)
            .map(|(i, _)| i)
            .collect();

        let mut order = Vec::with_capacity(n);
        while let Some(node) = queue.pop() {
            order.push(node);
            for &next in &adj[node] {
                in_degree[next] = in_degree[next].saturating_sub(1);
                if in_degree[next] == 0 {
                    queue.push(next);
                }
            }
        }

        if order.len() != n {
            return Err(GoalError::DependencyCycle(
                "cycle prevents topological ordering".to_string(),
            ));
        }

        Ok(order)
    }

    // -- Private helpers ----------------------------------------------------

    /// Try to match the goal against known templates.
    ///
    /// # Architecture note
    ///
    /// Template selection requires understanding natural language semantics —
    /// that belongs to the LLM brain, not Rust. Rust does not do keyword
    /// matching here. The LLM receives the goal description and the template
    /// catalogue and picks the appropriate template (or none) as part of the
    /// ReAct think step. This function always returns `None` so that the
    /// `decompose()` caller falls through to `flag_for_llm()`.
    fn try_template_decomposition(&mut self, _goal: &Goal) -> Option<DecompositionResult> {
        // LLM decides which template (if any) applies — Rust does not do NLP.
        None
    }

    /// Convert a goal's existing steps into sub-goals (ETG-guided).
    fn from_existing_steps(&mut self, goal: &Goal) -> DecompositionResult {
        let base_id = self.next_sub_goal_id;
        let mut sub_goals = Vec::with_capacity(goal.steps.len().min(MAX_SUB_GOALS));

        for (i, step) in goal.steps.iter().enumerate() {
            if i >= MAX_SUB_GOALS {
                break;
            }

            let sub = Goal {
                id: base_id + i as u64,
                description: step.description.clone(),
                priority: goal.priority,
                status: GoalStatus::Pending,
                steps: vec![step.clone()],
                created_ms: goal.created_ms,
                deadline_ms: goal.deadline_ms,
                parent_goal: Some(goal.id),
                source: GoalSource::GoalDecomposition(goal.id),
            };

            // Linear dependency chain: each step depends on the previous.
            let deps = if i > 0 { vec![i - 1] } else { vec![] };

            sub_goals.push(SubGoal {
                goal: sub,
                depends_on: deps,
                index: i,
            });
        }

        self.next_sub_goal_id = base_id + sub_goals.len() as u64;

        let estimated_duration: u32 = goal
            .steps
            .iter()
            .filter_map(|s| s.action.as_ref().map(|a| a.default_timeout()))
            .sum();

        DecompositionResult {
            parent_goal_id: goal.id,
            strategy: DecompositionStrategy::EtgGuided,
            sub_goals,
            confidence: 0.75, // Moderate confidence for ETG-derived plans.
            estimated_duration_ms: estimated_duration,
        }
    }

    /// Flag a goal for LLM-assisted decomposition.
    ///
    /// Returns a single "plan this goal" sub-goal that the neocortex will handle.
    fn flag_for_llm(&mut self, goal: &Goal) -> DecompositionResult {
        let sub_goal_id = self.next_sub_goal_id;
        self.next_sub_goal_id += 1;

        let sub = Goal {
            id: sub_goal_id,
            description: format!("Plan decomposition for: {}", goal.description),
            priority: goal.priority,
            status: GoalStatus::Pending,
            steps: vec![],
            created_ms: goal.created_ms,
            deadline_ms: goal.deadline_ms,
            parent_goal: Some(goal.id),
            source: GoalSource::GoalDecomposition(goal.id),
        };

        DecompositionResult {
            parent_goal_id: goal.id,
            strategy: DecompositionStrategy::LlmAssisted,
            sub_goals: vec![SubGoal {
                goal: sub,
                depends_on: vec![],
                index: 0,
            }],
            confidence: 0.5,          // Low confidence — needs LLM.
            estimated_duration_ms: 0, // Unknown until LLM plans.
        }
    }

    /// Register built-in decomposition templates for common goal patterns.
    fn register_builtin_templates(&mut self) {
        // Template: Send a message via messaging app.
        let send_message = DecompositionTemplate {
            id: "send_message".to_string(),
            trigger_keywords: vec![
                "send".to_string(),
                "message".to_string(),
                "text".to_string(),
                "whatsapp".to_string(),
                "telegram".to_string(),
            ],
            required_app: None,
            steps: vec![
                StepTemplate {
                    description: "Open messaging app".to_string(),
                    action: Some(ActionType::OpenApp {
                        package: "com.whatsapp".to_string(),
                    }),
                    max_attempts: 3,
                },
                StepTemplate {
                    description: "Search for contact".to_string(),
                    action: Some(ActionType::Tap { x: 0, y: 0 }), // Placeholder coords.
                    max_attempts: 3,
                },
                StepTemplate {
                    description: "Type message".to_string(),
                    action: Some(ActionType::Type {
                        text: String::new(),
                    }),
                    max_attempts: 2,
                },
                StepTemplate {
                    description: "Send message".to_string(),
                    action: Some(ActionType::Tap { x: 0, y: 0 }),
                    max_attempts: 3,
                },
            ],
            dependencies: vec![(0, 1), (1, 2), (2, 3)], // Linear chain.
            success_rate: 0.85,
        };

        // Template: Make a phone call.
        let make_call = DecompositionTemplate {
            id: "make_call".to_string(),
            trigger_keywords: vec![
                "call".to_string(),
                "phone".to_string(),
                "dial".to_string(),
                "ring".to_string(),
            ],
            required_app: None,
            steps: vec![
                StepTemplate {
                    description: "Open dialer app".to_string(),
                    action: Some(ActionType::OpenApp {
                        package: "com.android.dialer".to_string(),
                    }),
                    max_attempts: 3,
                },
                StepTemplate {
                    description: "Enter phone number or search contact".to_string(),
                    action: Some(ActionType::Type {
                        text: String::new(),
                    }),
                    max_attempts: 2,
                },
                StepTemplate {
                    description: "Tap call button".to_string(),
                    action: Some(ActionType::Tap { x: 0, y: 0 }),
                    max_attempts: 2,
                },
            ],
            dependencies: vec![(0, 1), (1, 2)],
            success_rate: 0.90,
        };

        // Template: Set alarm / timer.
        let set_alarm = DecompositionTemplate {
            id: "set_alarm".to_string(),
            trigger_keywords: vec![
                "alarm".to_string(),
                "timer".to_string(),
                "wake".to_string(),
                "remind".to_string(),
            ],
            required_app: None,
            steps: vec![
                StepTemplate {
                    description: "Open clock app".to_string(),
                    action: Some(ActionType::OpenApp {
                        package: "com.google.android.deskclock".to_string(),
                    }),
                    max_attempts: 3,
                },
                StepTemplate {
                    description: "Navigate to alarm/timer tab".to_string(),
                    action: Some(ActionType::Tap { x: 0, y: 0 }),
                    max_attempts: 2,
                },
                StepTemplate {
                    description: "Set time value".to_string(),
                    action: Some(ActionType::Type {
                        text: String::new(),
                    }),
                    max_attempts: 2,
                },
                StepTemplate {
                    description: "Confirm and save".to_string(),
                    action: Some(ActionType::Tap { x: 0, y: 0 }),
                    max_attempts: 2,
                },
            ],
            dependencies: vec![(0, 1), (1, 2), (2, 3)],
            success_rate: 0.88,
        };

        // Template: Web search.
        let web_search = DecompositionTemplate {
            id: "web_search".to_string(),
            trigger_keywords: vec![
                "search".to_string(),
                "google".to_string(),
                "look up".to_string(),
                "find".to_string(),
                "browse".to_string(),
            ],
            required_app: None,
            steps: vec![
                StepTemplate {
                    description: "Open browser".to_string(),
                    action: Some(ActionType::OpenApp {
                        package: "com.android.chrome".to_string(),
                    }),
                    max_attempts: 3,
                },
                StepTemplate {
                    description: "Tap URL bar".to_string(),
                    action: Some(ActionType::Tap { x: 0, y: 0 }),
                    max_attempts: 2,
                },
                StepTemplate {
                    description: "Type search query".to_string(),
                    action: Some(ActionType::Type {
                        text: String::new(),
                    }),
                    max_attempts: 2,
                },
            ],
            dependencies: vec![(0, 1), (1, 2)],
            success_rate: 0.82,
        };

        // Ignore capacity errors for built-in templates — should always fit.
        let _ = self.templates.try_push(send_message);
        let _ = self.templates.try_push(make_call);
        let _ = self.templates.try_push(set_alarm);
        let _ = self.templates.try_push(web_search);
    }
}

impl Default for GoalDecomposer {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// HTN (Hierarchical Task Network) Enhancement
// ===========================================================================

/// Maximum methods per compound task in the HTN method library.
const MAX_METHODS_PER_TASK: usize = 8;

/// Maximum compound tasks in the method library.
const MAX_COMPOUND_TASKS: usize = 64;

/// Maximum nodes in a partial-order plan.
#[allow(dead_code)] // Phase 8: used by HTN planner plan depth bound
const MAX_PLAN_NODES: usize = 32;

/// Maximum conditional branches in a plan.
const MAX_BRANCHES: usize = 8;

// ---------------------------------------------------------------------------
// HTN Task Types
// ---------------------------------------------------------------------------

/// Distinguishes primitive (directly executable) from compound (needs further
/// decomposition) tasks in the HTN.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HtnTaskKind {
    /// Directly maps to a single action — no further decomposition.
    Primitive,
    /// Must be decomposed via one of its registered methods.
    Compound,
}

/// A task node in the HTN. Can be primitive or compound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtnTask {
    /// Unique task identifier.
    pub id: String,
    /// Whether this is primitive or compound.
    pub kind: HtnTaskKind,
    /// Human-readable description.
    pub description: String,
    /// The action to execute (only meaningful for primitive tasks).
    pub action: Option<ActionType>,
    /// Estimated duration in milliseconds.
    pub estimated_duration_ms: u32,
    /// Preconditions that must hold before this task can execute (keyword tags).
    pub preconditions: Vec<String>,
    /// Effects produced after task completion (keyword tags).
    pub effects: Vec<String>,
}

/// A method for decomposing a compound task into sub-tasks.
///
/// Multiple methods can exist for the same compound task — the planner selects
/// the best one based on applicability and confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtnMethod {
    /// Which compound task this method decomposes.
    pub task_id: String,
    /// Human-readable method name.
    pub name: String,
    /// Selection strategy that produced this method.
    pub source: MethodSource,
    /// Ordered list of sub-task IDs produced by this method.
    pub sub_task_ids: Vec<String>,
    /// Dependencies among the sub-tasks as (from_idx, to_idx) pairs.
    pub ordering_constraints: Vec<(usize, usize)>,
    /// Applicability conditions — keyword tags that must be present in the goal.
    pub applicability: Vec<String>,
    /// Historical success rate (0.0–1.0).
    pub confidence: f32,
}

/// How a method was obtained.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MethodSource {
    /// Derived from ETG graph traversal.
    EtgBased,
    /// Generated by the LLM / Neocortex.
    LlmBased,
    /// Learned from successful past executions.
    Learned,
    /// Hand-authored built-in.
    Builtin,
}

/// A node in a partial-order plan (POP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanNode {
    /// Index of this node in the plan.
    pub index: usize,
    /// The HTN task at this node.
    pub task: HtnTask,
    /// Indices of nodes this one depends on (must complete before this starts).
    pub predecessors: Vec<usize>,
    /// Whether this node has been completed.
    pub completed: bool,
}

/// A conditional branch point in a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalBranch {
    /// Index of the branch decision node.
    pub decision_node: usize,
    /// Condition keyword — if the runtime state contains this tag, take `then_nodes`.
    pub condition: String,
    /// Node indices to execute if condition is true.
    pub then_nodes: Vec<usize>,
    /// Node indices to execute if condition is false.
    pub else_nodes: Vec<usize>,
}

/// A partial-order plan produced by HTN decomposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialOrderPlan {
    /// The goal this plan was produced for.
    pub goal_id: u64,
    /// All nodes in the plan.
    pub nodes: Vec<PlanNode>,
    /// Conditional branches (if any).
    pub branches: Vec<ConditionalBranch>,
    /// Overall confidence in this plan (0.0–1.0).
    pub confidence: f32,
    /// Estimated total duration (ms), accounting for parallelism.
    pub estimated_duration_ms: u32,
}

/// Result of identifying parallelizable sub-tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelGroup {
    /// Execution wave index (0 = first wave, 1 = second, …).
    pub wave: usize,
    /// Node indices that can execute in parallel within this wave.
    pub node_indices: Vec<usize>,
}

// ---------------------------------------------------------------------------
// HTN Method Library
// ---------------------------------------------------------------------------

/// The method library — stores compound task definitions and their methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodLibrary {
    /// Compound task definitions keyed by task ID.
    tasks: BoundedMap<String, HtnTask, MAX_COMPOUND_TASKS>,
    /// Methods keyed by the compound task ID they decompose.
    /// Each task can have up to `MAX_METHODS_PER_TASK` methods.
    methods: BoundedMap<String, BoundedVec<HtnMethod, MAX_METHODS_PER_TASK>, MAX_COMPOUND_TASKS>,
}

impl MethodLibrary {
    /// Create an empty method library.
    pub fn new() -> Self {
        Self {
            tasks: BoundedMap::new(),
            methods: BoundedMap::new(),
        }
    }

    /// Register a compound task definition.
    pub fn register_task(&mut self, task: HtnTask) -> Result<(), GoalError> {
        self.tasks
            .try_insert(task.id.clone(), task)
            .map_err(|_| GoalError::CapacityExceeded {
                max: MAX_COMPOUND_TASKS,
            })?;
        Ok(())
    }

    /// Add a decomposition method for a compound task.
    pub fn add_method(&mut self, method: HtnMethod) -> Result<(), GoalError> {
        let task_id = method.task_id.clone();
        if let Some(methods_vec) = self.methods.get_mut(&task_id) {
            methods_vec
                .try_push(method)
                .map_err(|_| GoalError::CapacityExceeded {
                    max: MAX_METHODS_PER_TASK,
                })?;
        } else {
            let mut methods_vec = BoundedVec::new();
            methods_vec
                .try_push(method)
                .map_err(|_| GoalError::CapacityExceeded {
                    max: MAX_METHODS_PER_TASK,
                })?;
            self.methods.try_insert(task_id, methods_vec).map_err(|_| {
                GoalError::CapacityExceeded {
                    max: MAX_COMPOUND_TASKS,
                }
            })?;
        }
        Ok(())
    }

    /// Look up a task definition by ID.
    pub fn get_task(&self, id: &str) -> Option<&HtnTask> {
        self.tasks.get(&id.to_string())
    }

    /// Select the best applicable method for a compound task given context keywords.
    ///
    /// Sorts applicable methods by confidence descending and returns the best.
    pub fn select_method(&self, task_id: &str, context_keywords: &[String]) -> Option<&HtnMethod> {
        let methods = self.methods.get(&task_id.to_string())?;
        let mut best: Option<&HtnMethod> = None;
        let mut best_score: f32 = -1.0;

        for method in methods.iter() {
            // Check applicability: at least one keyword must match (or empty = always applicable).
            let applicable = method.applicability.is_empty()
                || method.applicability.iter().any(|kw| {
                    context_keywords
                        .iter()
                        .any(|ctx| ctx.to_ascii_lowercase().contains(&kw.to_ascii_lowercase()))
                });

            if applicable && method.confidence > best_score {
                best_score = method.confidence;
                best = Some(method);
            }
        }

        best
    }

    /// Number of registered compound tasks.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Number of methods registered across all tasks.
    pub fn method_count(&self) -> usize {
        let mut total = 0;
        for (_, methods) in self.methods.iter() {
            total += methods.len();
        }
        total
    }

    /// Update a method's confidence based on execution outcome.
    pub fn record_method_outcome(&mut self, task_id: &str, method_name: &str, success: bool) {
        if let Some(methods) = self.methods.get_mut(&task_id.to_string()) {
            for method in methods.iter_mut() {
                if method.name == method_name {
                    if success {
                        method.confidence = (method.confidence + 0.02).min(0.99);
                    } else {
                        method.confidence = (method.confidence * 0.9).max(0.01);
                    }
                    break;
                }
            }
        }
    }
}

impl Default for MethodLibrary {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// HTN Decomposer
// ---------------------------------------------------------------------------

/// Full HTN decomposer wrapping the base `GoalDecomposer` with method library,
/// partial-order planning, plan refinement, and conditional branches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtnDecomposer {
    /// The base template-based decomposer.
    pub base: GoalDecomposer,
    /// Method library for compound task decomposition.
    pub library: MethodLibrary,
    /// Maximum decomposition depth.
    max_depth: usize,
    /// Counter for generating unique plan node IDs.
    next_node_id: usize,
}

impl HtnDecomposer {
    /// Create a new HTN decomposer with defaults.
    pub fn new() -> Self {
        Self {
            base: GoalDecomposer::new(),
            library: MethodLibrary::new(),
            max_depth: DEFAULT_MAX_DEPTH,
            next_node_id: 0,
        }
    }

    /// Decompose a goal into a partial-order plan using HTN methods.
    ///
    /// Falls back to the base decomposer if no compound task matches.
    #[instrument(skip(self), fields(goal_id = goal.id))]
    pub fn decompose_to_pop(&mut self, goal: &Goal) -> Result<PartialOrderPlan, GoalError> {
        self.next_node_id = 0;

        // Try to find a compound task that matches this goal description.
        let context_keywords: Vec<String> = goal
            .description
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();

        // Search the method library for a matching compound task.
        let mut matched_task_id: Option<String> = None;
        for (task_id, task) in self.library.tasks.iter() {
            if task.kind == HtnTaskKind::Compound {
                let desc_lower = goal.description.to_ascii_lowercase();
                let task_desc_lower = task.description.to_ascii_lowercase();
                // Match if any keyword from the task description appears in the goal.
                let task_keywords: Vec<&str> = task_desc_lower.split_whitespace().collect();
                let match_count = task_keywords
                    .iter()
                    .filter(|kw| desc_lower.contains(**kw))
                    .count();
                if match_count >= 2 || desc_lower.contains(&task_desc_lower) {
                    matched_task_id = Some(task_id.clone());
                    break;
                }
            }
        }

        if let Some(task_id) = matched_task_id {
            self.decompose_compound(&task_id, goal, &context_keywords, 0)
        } else {
            // Fall back to base decomposer and convert result to POP.
            let base_result = self.base.decompose(goal, 0)?;
            Ok(self.decomposition_result_to_pop(goal.id, &base_result))
        }
    }

    /// Recursively decompose a compound task using the method library.
    fn decompose_compound(
        &mut self,
        task_id: &str,
        goal: &Goal,
        context_keywords: &[String],
        depth: usize,
    ) -> Result<PartialOrderPlan, GoalError> {
        if depth > self.max_depth {
            return Err(GoalError::DecompositionFailed(format!(
                "HTN max depth {} exceeded at depth {}",
                self.max_depth, depth
            )));
        }

        let method = self
            .library
            .select_method(task_id, context_keywords)
            .ok_or_else(|| {
                GoalError::DecompositionFailed(format!("no applicable method for task {}", task_id))
            })?
            .clone();

        let mut nodes = Vec::with_capacity(method.sub_task_ids.len());
        let mut total_duration: u32 = 0;
        let mut min_confidence: f32 = method.confidence;

        for (i, sub_task_id) in method.sub_task_ids.iter().enumerate() {
            let task = self.library.get_task(sub_task_id).cloned().ok_or_else(|| {
                GoalError::DecompositionFailed(format!("sub-task {} not found", sub_task_id))
            })?;

            // Compute predecessors from ordering constraints.
            let predecessors: Vec<usize> = method
                .ordering_constraints
                .iter()
                .filter(|(_, to)| *to == i)
                .map(|(from, _)| *from)
                .collect();

            let node_idx = self.next_node_id;
            self.next_node_id += 1;

            total_duration = total_duration.saturating_add(task.estimated_duration_ms);
            if task.kind == HtnTaskKind::Compound {
                min_confidence = min_confidence.min(0.7); // Reduce confidence for unresolved
                                                          // compounds.
            }

            nodes.push(PlanNode {
                index: node_idx,
                task,
                predecessors,
                completed: false,
            });
        }

        Ok(PartialOrderPlan {
            goal_id: goal.id,
            nodes,
            branches: Vec::new(),
            confidence: min_confidence,
            estimated_duration_ms: total_duration,
        })
    }

    /// Convert a base `DecompositionResult` into a `PartialOrderPlan`.
    fn decomposition_result_to_pop(
        &mut self,
        goal_id: u64,
        result: &DecompositionResult,
    ) -> PartialOrderPlan {
        let nodes: Vec<PlanNode> = result
            .sub_goals
            .iter()
            .map(|sg| {
                let idx = self.next_node_id;
                self.next_node_id += 1;
                PlanNode {
                    index: idx,
                    task: HtnTask {
                        id: format!("sg_{}", sg.goal.id),
                        kind: HtnTaskKind::Primitive,
                        description: sg.goal.description.clone(),
                        action: sg.goal.steps.first().and_then(|s| s.action.clone()),
                        estimated_duration_ms: sg
                            .goal
                            .steps
                            .first()
                            .and_then(|s| s.action.as_ref().map(|a| a.default_timeout()))
                            .unwrap_or(3000),
                        preconditions: vec![],
                        effects: vec![],
                    },
                    predecessors: sg.depends_on.clone(),
                    completed: false,
                }
            })
            .collect();

        PartialOrderPlan {
            goal_id,
            nodes,
            branches: Vec::new(),
            confidence: result.confidence,
            estimated_duration_ms: result.estimated_duration_ms,
        }
    }

    /// Identify parallel execution groups (waves) from a partial-order plan.
    ///
    /// Uses topological sort to group nodes into waves where all nodes
    /// in a wave have their predecessors completed in earlier waves.
    pub fn identify_parallel_groups(
        &self,
        plan: &PartialOrderPlan,
    ) -> Result<Vec<ParallelGroup>, GoalError> {
        let n = plan.nodes.len();
        if n == 0 {
            return Ok(vec![]);
        }

        // Build in-degree map.
        let mut in_degree = vec![0u32; n];
        let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

        for node in &plan.nodes {
            for &pred in &node.predecessors {
                if pred >= n {
                    return Err(GoalError::DependencyCycle(format!(
                        "predecessor index {} out of range",
                        pred
                    )));
                }
                adj[pred].push(node.index);
                in_degree[node.index] = in_degree[node.index].saturating_add(1);
            }
        }

        let mut waves = Vec::new();
        let mut remaining = n;
        let mut current_in_degree = in_degree;

        while remaining > 0 {
            // Collect all nodes with zero in-degree.
            let ready: Vec<usize> = current_in_degree
                .iter()
                .enumerate()
                .filter(|(_, &d)| d == 0)
                .map(|(i, _)| i)
                .collect();

            if ready.is_empty() {
                return Err(GoalError::DependencyCycle(
                    "cycle detected during parallel group identification".to_string(),
                ));
            }

            let wave_idx = waves.len();
            waves.push(ParallelGroup {
                wave: wave_idx,
                node_indices: ready.clone(),
            });

            // "Remove" processed nodes by setting in-degree to u32::MAX.
            for &node_idx in &ready {
                current_in_degree[node_idx] = u32::MAX; // Mark as processed.
                for &successor in &adj[node_idx] {
                    current_in_degree[successor] = current_in_degree[successor].saturating_sub(1);
                }
            }

            remaining -= ready.len();
        }

        Ok(waves)
    }

    /// Validate a partial-order plan's DAG structure.
    pub fn validate_pop(&self, plan: &PartialOrderPlan) -> Result<(), GoalError> {
        let n = plan.nodes.len();
        if n == 0 {
            return Ok(());
        }

        let mut in_degree = vec![0u32; n];
        let mut adj: Vec<Vec<usize>> = vec![vec![]; n];

        for node in &plan.nodes {
            for &pred in &node.predecessors {
                if pred >= n {
                    return Err(GoalError::DependencyCycle(format!(
                        "predecessor {} out of range (max {})",
                        pred,
                        n - 1
                    )));
                }
                adj[pred].push(node.index);
                in_degree[node.index] = in_degree[node.index].saturating_add(1);
            }
        }

        // Kahn's algorithm.
        let mut queue: Vec<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &d)| d == 0)
            .map(|(i, _)| i)
            .collect();

        let mut visited = 0usize;
        while let Some(node) = queue.pop() {
            visited += 1;
            for &next in &adj[node] {
                in_degree[next] = in_degree[next].saturating_sub(1);
                if in_degree[next] == 0 {
                    queue.push(next);
                }
            }
        }

        if visited != n {
            Err(GoalError::DependencyCycle(format!(
                "cycle in POP: visited {} of {} nodes",
                visited, n
            )))
        } else {
            Ok(())
        }
    }

    /// Refine a plan by re-decomposing a failed node.
    ///
    /// Replaces the failed node with a fresh decomposition attempt,
    /// preserving the rest of the plan structure.
    pub fn refine_failed_node(
        &mut self,
        plan: &mut PartialOrderPlan,
        failed_index: usize,
        goal: &Goal,
    ) -> Result<(), GoalError> {
        if failed_index >= plan.nodes.len() {
            return Err(GoalError::NotFound(failed_index as u64));
        }

        let failed_node = &plan.nodes[failed_index];
        if failed_node.task.kind == HtnTaskKind::Primitive {
            // For primitive tasks, mark as failed and flag for LLM re-plan.
            tracing::warn!(
                node_index = failed_index,
                task_id = %failed_node.task.id,
                "primitive task failed — flagging for re-plan"
            );
            // Create a replacement LLM-assisted task.
            let replacement = HtnTask {
                id: format!("{}_retry", failed_node.task.id),
                kind: HtnTaskKind::Primitive,
                description: format!("Retry: {}", failed_node.task.description),
                action: failed_node.task.action.clone(),
                estimated_duration_ms: failed_node.task.estimated_duration_ms,
                preconditions: failed_node.task.preconditions.clone(),
                effects: failed_node.task.effects.clone(),
            };
            plan.nodes[failed_index].task = replacement;
            plan.nodes[failed_index].completed = false;
            plan.confidence *= 0.8; // Reduce confidence after failure.
            return Ok(());
        }

        // For compound tasks, try alternative method.
        let context_keywords: Vec<String> = goal
            .description
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();

        let task_id = failed_node.task.id.clone();
        let sub_plan = self.decompose_compound(&task_id, goal, &context_keywords, 1)?;

        // Replace the single failed node with the expanded sub-plan nodes.
        // Re-wire predecessors: the new nodes inherit the failed node's predecessors.
        let original_predecessors = plan.nodes[failed_index].predecessors.clone();
        plan.nodes[failed_index] = sub_plan.nodes.first().cloned().ok_or_else(|| {
            GoalError::DecompositionFailed("empty sub-plan during refinement".to_string())
        })?;
        plan.nodes[failed_index].predecessors = original_predecessors;
        plan.confidence = (plan.confidence * 0.9).min(sub_plan.confidence);

        Ok(())
    }

    /// Add a conditional branch to a plan.
    pub fn add_branch(
        &self,
        plan: &mut PartialOrderPlan,
        branch: ConditionalBranch,
    ) -> Result<(), GoalError> {
        if plan.branches.len() >= MAX_BRANCHES {
            return Err(GoalError::CapacityExceeded { max: MAX_BRANCHES });
        }
        if branch.decision_node >= plan.nodes.len() {
            return Err(GoalError::NotFound(branch.decision_node as u64));
        }
        plan.branches.push(branch);
        Ok(())
    }

    /// Compute the critical path duration through a POP (longest path).
    pub fn critical_path_duration(&self, plan: &PartialOrderPlan) -> u32 {
        let n = plan.nodes.len();
        if n == 0 {
            return 0;
        }

        // Dynamic programming: longest path in DAG.
        let mut longest = vec![0u32; n];
        // Process in topological order.
        let waves = match self.identify_parallel_groups(plan) {
            Ok(w) => w,
            Err(_) => return plan.estimated_duration_ms, // Fallback on cycle.
        };

        for wave in &waves {
            for &idx in &wave.node_indices {
                let node_duration = plan.nodes[idx].task.estimated_duration_ms;
                let max_pred = plan.nodes[idx]
                    .predecessors
                    .iter()
                    .map(|&p| longest.get(p).copied().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                longest[idx] = max_pred.saturating_add(node_duration);
            }
        }

        longest.iter().copied().max().unwrap_or(0)
    }
}

impl Default for HtnDecomposer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use aura_types::goals::{GoalPriority, GoalSource, GoalStatus};

    use super::*;

    fn make_goal(id: u64, description: &str) -> Goal {
        Goal {
            id,
            description: description.to_string(),
            priority: GoalPriority::Medium,
            status: GoalStatus::Pending,
            steps: vec![],
            created_ms: 1_700_000_000_000,
            deadline_ms: None,
            parent_goal: None,
            source: GoalSource::UserExplicit,
        }
    }

    #[test]
    fn test_template_keyword_matching_never_fires() {
        // Architecture: Rust does not do NLP keyword matching to select templates.
        // All goals that lack pre-built steps fall through to LlmAssisted,
        // so the LLM brain can select the appropriate template (or none).
        let mut decomposer = GoalDecomposer::new();
        let goal = make_goal(1, "Send a WhatsApp message to Alice");

        let result = decomposer
            .decompose(&goal, 0)
            .expect("decomposition should succeed");

        // No keyword matching — falls through to LLM.
        assert_eq!(result.strategy, DecompositionStrategy::LlmAssisted);
        assert_eq!(result.sub_goals.len(), 1);
        assert_eq!(result.parent_goal_id, 1);
    }

    #[test]
    fn test_etg_guided_decomposition() {
        let mut decomposer = GoalDecomposer::new();
        let mut goal = make_goal(2, "Custom task with existing steps");
        goal.steps = vec![
            GoalStep {
                index: 0,
                description: "Step A".to_string(),
                action: Some(ActionType::Tap { x: 100, y: 200 }),
                status: StepStatus::Pending,
                attempts: 0,
                max_attempts: 3,
            },
            GoalStep {
                index: 1,
                description: "Step B".to_string(),
                action: Some(ActionType::Back),
                status: StepStatus::Pending,
                attempts: 0,
                max_attempts: 3,
            },
        ];

        let result = decomposer
            .decompose(&goal, 0)
            .expect("decomposition should succeed");

        assert_eq!(result.strategy, DecompositionStrategy::EtgGuided);
        assert_eq!(result.sub_goals.len(), 2);
        // Linear dependency: step 1 depends on step 0.
        assert_eq!(result.sub_goals[1].depends_on, vec![0]);
    }

    #[test]
    fn test_llm_fallback_for_unknown_goal() {
        let mut decomposer = GoalDecomposer::new();
        let goal = make_goal(3, "Analyze my spending patterns and create a budget");

        let result = decomposer
            .decompose(&goal, 0)
            .expect("decomposition should succeed");

        assert_eq!(result.strategy, DecompositionStrategy::LlmAssisted);
        assert_eq!(result.sub_goals.len(), 1);
        assert_eq!(result.confidence, 0.5);
    }

    #[test]
    fn test_max_depth_exceeded() {
        let mut decomposer = GoalDecomposer::with_max_depth(2);
        let goal = make_goal(4, "Something");

        let result = decomposer.decompose(&goal, 3);
        assert!(result.is_err());
        match result {
            Err(GoalError::DecompositionFailed(msg)) => {
                assert!(msg.contains("max depth"));
            }
            other => panic!("expected DecompositionFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_dag_validation_valid() {
        let decomposer = GoalDecomposer::new();
        let result = DecompositionResult {
            parent_goal_id: 1,
            strategy: DecompositionStrategy::TemplateBased,
            sub_goals: vec![
                SubGoal {
                    goal: make_goal(10, "A"),
                    depends_on: vec![],
                    index: 0,
                },
                SubGoal {
                    goal: make_goal(11, "B"),
                    depends_on: vec![0],
                    index: 1,
                },
                SubGoal {
                    goal: make_goal(12, "C"),
                    depends_on: vec![0],
                    index: 2,
                },
                SubGoal {
                    goal: make_goal(13, "D"),
                    depends_on: vec![1, 2],
                    index: 3,
                },
            ],
            confidence: 0.8,
            estimated_duration_ms: 5000,
        };

        assert!(decomposer.validate_dag(&result).is_ok());

        let order = decomposer
            .topological_order(&result)
            .expect("should succeed");
        assert_eq!(order.len(), 4);
        // 0 must come before 1, 2, 3.
        let pos_0 = order.iter().position(|&x| x == 0).expect("should find 0");
        let pos_3 = order.iter().position(|&x| x == 3).expect("should find 3");
        assert!(pos_0 < pos_3);
    }

    #[test]
    fn test_dag_validation_cycle() {
        let decomposer = GoalDecomposer::new();
        let result = DecompositionResult {
            parent_goal_id: 1,
            strategy: DecompositionStrategy::TemplateBased,
            sub_goals: vec![
                SubGoal {
                    goal: make_goal(10, "A"),
                    depends_on: vec![1], // A depends on B.
                    index: 0,
                },
                SubGoal {
                    goal: make_goal(11, "B"),
                    depends_on: vec![0], // B depends on A → cycle!
                    index: 1,
                },
            ],
            confidence: 0.8,
            estimated_duration_ms: 1000,
        };

        assert!(decomposer.validate_dag(&result).is_err());
    }

    #[test]
    fn test_is_atomic() {
        let decomposer = GoalDecomposer::new();

        let atomic = make_goal(1, "Simple task");
        assert!(decomposer.is_atomic(&atomic));

        let mut complex = make_goal(2, "Multi-step task");
        complex.steps = vec![GoalStep::default(), GoalStep::default()];
        assert!(!decomposer.is_atomic(&complex));
    }

    #[test]
    fn test_builtin_templates_loaded() {
        let decomposer = GoalDecomposer::new();
        assert!(
            decomposer.template_count() >= 4,
            "should have at least 4 built-in templates, got {}",
            decomposer.template_count()
        );
    }

    // ===================================================================
    // HTN Enhancement Tests (25 new tests)
    // ===================================================================

    fn make_primitive_task(id: &str, desc: &str) -> HtnTask {
        HtnTask {
            id: id.to_string(),
            kind: HtnTaskKind::Primitive,
            description: desc.to_string(),
            action: Some(ActionType::Tap { x: 0, y: 0 }),
            estimated_duration_ms: 1000,
            preconditions: vec![],
            effects: vec![],
        }
    }

    fn make_compound_task(id: &str, desc: &str) -> HtnTask {
        HtnTask {
            id: id.to_string(),
            kind: HtnTaskKind::Compound,
            description: desc.to_string(),
            action: None,
            estimated_duration_ms: 0,
            preconditions: vec![],
            effects: vec![],
        }
    }

    fn setup_library_with_send_message() -> MethodLibrary {
        let mut lib = MethodLibrary::new();

        // Register primitive tasks.
        let _ = lib.register_task(make_primitive_task("open_app", "Open messaging app"));
        let _ = lib.register_task(make_primitive_task("find_contact", "Find contact"));
        let _ = lib.register_task(make_primitive_task("type_msg", "Type message"));
        let _ = lib.register_task(make_primitive_task("tap_send", "Tap send button"));

        // Register compound task.
        let _ = lib.register_task(make_compound_task(
            "send_message",
            "send a message to contact",
        ));

        // Register method.
        let _ = lib.add_method(HtnMethod {
            task_id: "send_message".to_string(),
            name: "via_whatsapp".to_string(),
            source: MethodSource::Builtin,
            sub_task_ids: vec![
                "open_app".to_string(),
                "find_contact".to_string(),
                "type_msg".to_string(),
                "tap_send".to_string(),
            ],
            ordering_constraints: vec![(0, 1), (1, 2), (2, 3)],
            applicability: vec!["message".to_string(), "send".to_string()],
            confidence: 0.85,
        });

        lib
    }

    #[test]
    fn test_method_library_creation() {
        let lib = MethodLibrary::new();
        assert_eq!(lib.task_count(), 0);
        assert_eq!(lib.method_count(), 0);
    }

    #[test]
    fn test_method_library_register_task() {
        let mut lib = MethodLibrary::new();
        let task = make_primitive_task("tap_button", "Tap the button");
        lib.register_task(task).expect("should register");
        assert_eq!(lib.task_count(), 1);
        assert!(lib.get_task("tap_button").is_some());
    }

    #[test]
    fn test_method_library_add_method() {
        let mut lib = MethodLibrary::new();
        let _ = lib.register_task(make_compound_task("parent", "parent task"));
        let _ = lib.register_task(make_primitive_task("child_a", "child A"));

        let method = HtnMethod {
            task_id: "parent".to_string(),
            name: "method_1".to_string(),
            source: MethodSource::Builtin,
            sub_task_ids: vec!["child_a".to_string()],
            ordering_constraints: vec![],
            applicability: vec![],
            confidence: 0.9,
        };
        lib.add_method(method).expect("should add method");
        assert_eq!(lib.method_count(), 1);
    }

    #[test]
    fn test_method_library_select_method_by_keyword() {
        let lib = setup_library_with_send_message();
        let context = vec!["send".to_string(), "message".to_string()];
        let method = lib.select_method("send_message", &context);
        assert!(method.is_some());
        assert_eq!(method.unwrap().name, "via_whatsapp");
    }

    #[test]
    fn test_method_library_no_method_for_unknown_task() {
        let lib = setup_library_with_send_message();
        let context = vec!["send".to_string()];
        let method = lib.select_method("unknown_task", &context);
        assert!(method.is_none());
    }

    #[test]
    fn test_method_library_record_outcome_success() {
        let mut lib = setup_library_with_send_message();
        let original_conf = lib
            .select_method("send_message", &["send".to_string()])
            .unwrap()
            .confidence;
        lib.record_method_outcome("send_message", "via_whatsapp", true);
        let new_conf = lib
            .select_method("send_message", &["send".to_string()])
            .unwrap()
            .confidence;
        assert!(new_conf > original_conf);
    }

    #[test]
    fn test_method_library_record_outcome_failure() {
        let mut lib = setup_library_with_send_message();
        let original_conf = lib
            .select_method("send_message", &["send".to_string()])
            .unwrap()
            .confidence;
        lib.record_method_outcome("send_message", "via_whatsapp", false);
        let new_conf = lib
            .select_method("send_message", &["send".to_string()])
            .unwrap()
            .confidence;
        assert!(new_conf < original_conf);
    }

    #[test]
    fn test_htn_decomposer_creation() {
        let htn = HtnDecomposer::new();
        assert_eq!(htn.library.task_count(), 0);
        assert!(htn.base.template_count() >= 4);
    }

    #[test]
    fn test_htn_decompose_fallback_to_base() {
        let mut htn = HtnDecomposer::new();
        let goal = make_goal(1, "Send a WhatsApp message to Alice");
        let pop = htn.decompose_to_pop(&goal).expect("should decompose");
        // Falls back to base template decomposer since library is empty.
        assert!(!pop.nodes.is_empty());
        assert!(pop.confidence > 0.0);
    }

    #[test]
    fn test_htn_decompose_via_method_library() {
        let mut htn = HtnDecomposer::new();
        htn.library = setup_library_with_send_message();

        let goal = make_goal(1, "send a message to contact Alice");
        let pop = htn.decompose_to_pop(&goal).expect("should decompose");
        assert_eq!(pop.nodes.len(), 4); // open_app, find_contact, type_msg, tap_send
        assert_eq!(pop.goal_id, 1);
        assert!(pop.confidence >= 0.8);
    }

    #[test]
    fn test_htn_parallel_groups_linear_chain() {
        let mut htn = HtnDecomposer::new();
        htn.library = setup_library_with_send_message();

        let goal = make_goal(1, "send a message to contact Alice");
        let pop = htn.decompose_to_pop(&goal).expect("should decompose");
        let groups = htn.identify_parallel_groups(&pop).expect("should identify");

        // Linear chain → each wave has exactly 1 node.
        assert_eq!(groups.len(), 4);
        for group in &groups {
            assert_eq!(group.node_indices.len(), 1);
        }
    }

    #[test]
    fn test_htn_parallel_groups_with_parallelism() {
        let htn = HtnDecomposer::new();
        // Build a POP with parallel structure: A -> (B, C) -> D
        let pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![
                PlanNode {
                    index: 0,
                    task: make_primitive_task("a", "Task A"),
                    predecessors: vec![],
                    completed: false,
                },
                PlanNode {
                    index: 1,
                    task: make_primitive_task("b", "Task B"),
                    predecessors: vec![0],
                    completed: false,
                },
                PlanNode {
                    index: 2,
                    task: make_primitive_task("c", "Task C"),
                    predecessors: vec![0],
                    completed: false,
                },
                PlanNode {
                    index: 3,
                    task: make_primitive_task("d", "Task D"),
                    predecessors: vec![1, 2],
                    completed: false,
                },
            ],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 4000,
        };

        let groups = htn.identify_parallel_groups(&pop).expect("should identify");
        assert_eq!(groups.len(), 3); // Wave 0: [A], Wave 1: [B, C], Wave 2: [D]
        assert_eq!(groups[0].node_indices.len(), 1);
        assert_eq!(groups[1].node_indices.len(), 2); // B and C in parallel
        assert_eq!(groups[2].node_indices.len(), 1);
    }

    #[test]
    fn test_htn_validate_pop_valid() {
        let htn = HtnDecomposer::new();
        let pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![
                PlanNode {
                    index: 0,
                    task: make_primitive_task("a", "A"),
                    predecessors: vec![],
                    completed: false,
                },
                PlanNode {
                    index: 1,
                    task: make_primitive_task("b", "B"),
                    predecessors: vec![0],
                    completed: false,
                },
            ],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 2000,
        };
        assert!(htn.validate_pop(&pop).is_ok());
    }

    #[test]
    fn test_htn_validate_pop_cycle() {
        let htn = HtnDecomposer::new();
        let pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![
                PlanNode {
                    index: 0,
                    task: make_primitive_task("a", "A"),
                    predecessors: vec![1],
                    completed: false,
                },
                PlanNode {
                    index: 1,
                    task: make_primitive_task("b", "B"),
                    predecessors: vec![0],
                    completed: false,
                },
            ],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 2000,
        };
        assert!(htn.validate_pop(&pop).is_err());
    }

    #[test]
    fn test_htn_validate_pop_empty() {
        let htn = HtnDecomposer::new();
        let pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 0,
        };
        assert!(htn.validate_pop(&pop).is_ok());
    }

    #[test]
    fn test_htn_critical_path_linear() {
        let htn = HtnDecomposer::new();
        let pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![
                PlanNode {
                    index: 0,
                    task: make_primitive_task("a", "A"), // 1000ms
                    predecessors: vec![],
                    completed: false,
                },
                PlanNode {
                    index: 1,
                    task: make_primitive_task("b", "B"), // 1000ms
                    predecessors: vec![0],
                    completed: false,
                },
            ],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 2000,
        };
        // Critical path: A(1000) + B(1000) = 2000.
        assert_eq!(htn.critical_path_duration(&pop), 2000);
    }

    #[test]
    fn test_htn_critical_path_parallel() {
        let htn = HtnDecomposer::new();
        let mut task_b = make_primitive_task("b", "B");
        task_b.estimated_duration_ms = 2000;

        let pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![
                PlanNode {
                    index: 0,
                    task: make_primitive_task("a", "A"), // 1000ms
                    predecessors: vec![],
                    completed: false,
                },
                PlanNode {
                    index: 1,
                    task: task_b, // 2000ms, parallel with C
                    predecessors: vec![0],
                    completed: false,
                },
                PlanNode {
                    index: 2,
                    task: make_primitive_task("c", "C"), // 1000ms, parallel with B
                    predecessors: vec![0],
                    completed: false,
                },
                PlanNode {
                    index: 3,
                    task: make_primitive_task("d", "D"), // 1000ms
                    predecessors: vec![1, 2],
                    completed: false,
                },
            ],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 5000,
        };
        // Critical path: A(1000) + B(2000) + D(1000) = 4000 (not C which is 1000).
        assert_eq!(htn.critical_path_duration(&pop), 4000);
    }

    #[test]
    fn test_htn_refine_failed_primitive() {
        let mut htn = HtnDecomposer::new();
        let goal = make_goal(1, "do something");
        let mut pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![PlanNode {
                index: 0,
                task: make_primitive_task("step1", "Step 1"),
                predecessors: vec![],
                completed: false,
            }],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 1000,
        };

        htn.refine_failed_node(&mut pop, 0, &goal)
            .expect("should refine");
        // Task should be replaced with a retry variant.
        assert!(pop.nodes[0].task.id.contains("retry"));
        assert!(pop.confidence < 0.9); // Confidence reduced.
    }

    #[test]
    fn test_htn_refine_out_of_bounds() {
        let mut htn = HtnDecomposer::new();
        let goal = make_goal(1, "do something");
        let mut pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 0,
        };

        let result = htn.refine_failed_node(&mut pop, 5, &goal);
        assert!(result.is_err());
    }

    #[test]
    fn test_htn_add_branch() {
        let htn = HtnDecomposer::new();
        let mut pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![
                PlanNode {
                    index: 0,
                    task: make_primitive_task("a", "A"),
                    predecessors: vec![],
                    completed: false,
                },
                PlanNode {
                    index: 1,
                    task: make_primitive_task("b", "B"),
                    predecessors: vec![],
                    completed: false,
                },
            ],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 2000,
        };

        let branch = ConditionalBranch {
            decision_node: 0,
            condition: "app_open".to_string(),
            then_nodes: vec![1],
            else_nodes: vec![],
        };
        htn.add_branch(&mut pop, branch).expect("should add branch");
        assert_eq!(pop.branches.len(), 1);
    }

    #[test]
    fn test_htn_add_branch_invalid_decision_node() {
        let htn = HtnDecomposer::new();
        let mut pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 0,
        };

        let branch = ConditionalBranch {
            decision_node: 99,
            condition: "app_open".to_string(),
            then_nodes: vec![],
            else_nodes: vec![],
        };
        assert!(htn.add_branch(&mut pop, branch).is_err());
    }

    #[test]
    fn test_htn_task_kind_equality() {
        assert_eq!(HtnTaskKind::Primitive, HtnTaskKind::Primitive);
        assert_eq!(HtnTaskKind::Compound, HtnTaskKind::Compound);
        assert_ne!(HtnTaskKind::Primitive, HtnTaskKind::Compound);
    }

    #[test]
    fn test_method_source_equality() {
        assert_eq!(MethodSource::Builtin, MethodSource::Builtin);
        assert_ne!(MethodSource::EtgBased, MethodSource::LlmBased);
    }

    #[test]
    fn test_method_library_multiple_methods_per_task() {
        let mut lib = setup_library_with_send_message();
        // Add a second method for the same compound task.
        let method2 = HtnMethod {
            task_id: "send_message".to_string(),
            name: "via_telegram".to_string(),
            source: MethodSource::Learned,
            sub_task_ids: vec![
                "open_app".to_string(),
                "find_contact".to_string(),
                "type_msg".to_string(),
                "tap_send".to_string(),
            ],
            ordering_constraints: vec![(0, 1), (1, 2), (2, 3)],
            applicability: vec!["telegram".to_string()],
            confidence: 0.90,
        };
        lib.add_method(method2).expect("should add method");
        assert_eq!(lib.method_count(), 2);

        // When context says "telegram", should select the telegram method.
        let method = lib.select_method("send_message", &["telegram".to_string()]);
        assert!(method.is_some());
        assert_eq!(method.unwrap().name, "via_telegram");
    }

    #[test]
    fn test_htn_decompose_unknown_goal_fallback() {
        let mut htn = HtnDecomposer::new();
        htn.library = setup_library_with_send_message();

        // A goal that doesn't match any compound task → falls back to base.
        let goal = make_goal(99, "analyze my spending patterns and create a budget");
        let pop = htn.decompose_to_pop(&goal).expect("should decompose");
        // Falls back to LLM flagging via base decomposer.
        assert!(!pop.nodes.is_empty());
    }

    #[test]
    fn test_parallel_groups_detects_cycle() {
        let htn = HtnDecomposer::new();
        let pop = PartialOrderPlan {
            goal_id: 1,
            nodes: vec![
                PlanNode {
                    index: 0,
                    task: make_primitive_task("a", "A"),
                    predecessors: vec![1],
                    completed: false,
                },
                PlanNode {
                    index: 1,
                    task: make_primitive_task("b", "B"),
                    predecessors: vec![0],
                    completed: false,
                },
            ],
            branches: vec![],
            confidence: 0.9,
            estimated_duration_ms: 2000,
        };
        let result = htn.identify_parallel_groups(&pop);
        assert!(result.is_err());
    }

    #[test]
    fn test_htn_default_impl() {
        let htn = HtnDecomposer::default();
        assert_eq!(htn.max_depth, DEFAULT_MAX_DEPTH);
    }

    #[test]
    fn test_method_library_default_impl() {
        let lib = MethodLibrary::default();
        assert_eq!(lib.task_count(), 0);
    }
}
