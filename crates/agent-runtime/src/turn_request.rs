#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnGoal {
    pub objective: String,
}

impl TurnGoal {
    pub fn new(objective: impl Into<String>) -> Self {
        Self {
            objective: objective.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRequest {
    pub user_text: String,
    pub goal: Option<TurnGoal>,
    pub token_budget: Option<u64>,
    pub context_budget_bytes: Option<usize>,
}

impl TurnRequest {
    pub fn new(user_text: impl Into<String>) -> Self {
        Self {
            user_text: user_text.into(),
            goal: None,
            token_budget: None,
            context_budget_bytes: None,
        }
    }

    pub fn with_goal(mut self, goal: TurnGoal) -> Self {
        self.goal = Some(goal);
        self
    }

    pub fn with_token_budget(mut self, token_budget: u64) -> Self {
        self.token_budget = Some(token_budget);
        self
    }

    pub fn with_context_budget_bytes(mut self, context_budget_bytes: usize) -> Self {
        self.context_budget_bytes = Some(context_budget_bytes);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub exceeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetPolicy {
    token_budget: Option<u64>,
    total_tokens: u64,
}

impl BudgetPolicy {
    pub fn new(token_budget: Option<u64>) -> Self {
        Self {
            token_budget,
            total_tokens: 0,
        }
    }

    pub fn record_usage(&mut self, input_tokens: u64, output_tokens: u64) -> UsageSnapshot {
        let turn_tokens = input_tokens.saturating_add(output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(turn_tokens);
        UsageSnapshot {
            input_tokens,
            output_tokens,
            total_tokens: self.total_tokens,
            exceeded: self
                .token_budget
                .map(|budget| self.total_tokens > budget)
                .unwrap_or(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_policy_tracks_total_tokens() {
        let mut budget = BudgetPolicy::new(Some(10));

        assert!(!budget.record_usage(3, 4).exceeded);
        let usage = budget.record_usage(2, 2);

        assert_eq!(usage.total_tokens, 11);
        assert!(usage.exceeded);
    }
}
