use super::*;

#[test]
fn trigger_policy_matches_unicode_name_and_alias_in_chinese_sentence() {
    let catalog = SkillCatalog {
        root: None,
        entries: Vec::new(),
        summaries: vec![SkillSummary {
            name: "mail-assistant".into(),
            description: "Manage mail.".into(),
            aliases: vec!["邮件管理".into()],
            source: PathBuf::from("mail/SKILL.md"),
        }],
    };

    assert_eq!(
        catalog.triggered_skill_names("请使用邮件管理帮我整理收件箱。"),
        vec!["mail-assistant".to_string()]
    );
}

#[test]
fn explicit_unicode_invocation_is_deterministic() {
    let catalog = SkillCatalog {
        root: None,
        entries: Vec::new(),
        summaries: vec![SkillSummary {
            name: "记忆".into(),
            description: "管理长期记忆。".into(),
            aliases: Vec::new(),
            source: PathBuf::from("memory/SKILL.md"),
        }],
    };

    assert_eq!(
        catalog.triggered_skills("请调用 $记忆"),
        vec![SkillSelection {
            name: "记忆".into(),
            reason: SkillSelectionReason::ExplicitInvocation,
        }]
    );
}
