use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionResult {
    pub input: Vec<Value>,
    pub original_items: usize,
    pub compacted_items: usize,
    pub compacted: bool,
}

pub fn compact_model_input(input: Vec<Value>, budget_bytes: usize) -> anyhow::Result<Vec<Value>> {
    Ok(compact_model_input_with_stats(input, budget_bytes)?.input)
}

pub fn compact_model_input_with_stats(
    input: Vec<Value>,
    budget_bytes: usize,
) -> anyhow::Result<CompactionResult> {
    let original_items = input.len();
    if serialized_len(&input)? <= budget_bytes || input.len() <= 3 {
        return Ok(CompactionResult {
            compacted_items: input.len(),
            input,
            original_items,
            compacted: false,
        });
    }

    let last_index = input.len() - 1;
    let mut authority_end = 0;
    while authority_end < last_index && is_authority_item(&input[authority_end]) {
        authority_end += 1;
    }

    let mut compacted = Vec::new();
    compacted.extend(input[..authority_end].iter().cloned());
    let omitted_items = last_index.saturating_sub(authority_end);
    if omitted_items > 0 {
        compacted.push(json!({
            "role": "developer",
            "content": format!(
                "<context_compaction>\noriginal_items={original_items}\nomitted_items={omitted_items}\nOlder conversation items were omitted by deterministic budget compaction.\n</context_compaction>"
            )
        }));
    }
    compacted.push(input[last_index].clone());

    Ok(CompactionResult {
        original_items,
        compacted_items: compacted.len(),
        input: compacted,
        compacted: omitted_items > 0,
    })
}

fn is_authority_item(item: &Value) -> bool {
    matches!(
        item.get("role").and_then(Value::as_str),
        Some("system" | "developer")
    )
}

fn serialized_len<T: serde::Serialize + ?Sized>(value: &T) -> anyhow::Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

pub fn exceeds_budget(input: &[Value], budget_bytes: usize) -> anyhow::Result<bool> {
    Ok(serialized_len(input)? > budget_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compaction_preserves_authority_blocks_and_current_user() {
        let input = vec![
            json!({ "role": "system", "content": "system policy" }),
            json!({ "role": "developer", "content": "developer policy" }),
            json!({ "role": "user", "content": "old user" }),
            json!({ "role": "assistant", "content": "old answer" }),
            json!({ "role": "user", "content": "current user" }),
        ];

        let compacted = compact_model_input(input, 64).unwrap();

        assert_eq!(compacted[0]["content"], "system policy");
        assert_eq!(compacted[1]["content"], "developer policy");
        assert!(compacted.iter().any(|item| {
            item["content"]
                .as_str()
                .unwrap_or_default()
                .contains("<context_compaction>")
        }));
        assert_eq!(compacted.last().unwrap()["content"], "current user");
    }
}
