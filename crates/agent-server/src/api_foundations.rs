pub struct AppFoundationRuntimes {
    pub memory_tools: Option<agent_runtime::memory_tools::MemoryToolRuntime>,
    pub task_tools: Option<agent_runtime::task_tools::TaskToolRuntime>,
    pub automation_tools: Option<agent_runtime::automation_tools::AutomationToolRuntime>,
    pub attachment_tools: Option<agent_runtime::attachment_tools::AttachmentToolRuntime>,
    pub connector_tools: Option<agent_runtime::connector_tools::ConnectorToolRuntime>,
    pub mail_actions: Option<agent_runtime::foundation_actions::MailActionService>,
}

impl AppFoundationRuntimes {
    pub fn new(
        memory_tools: Option<agent_runtime::memory_tools::MemoryToolRuntime>,
        task_tools: Option<agent_runtime::task_tools::TaskToolRuntime>,
        connector_tools: Option<agent_runtime::connector_tools::ConnectorToolRuntime>,
    ) -> Self {
        Self {
            memory_tools,
            task_tools,
            automation_tools: None,
            attachment_tools: None,
            connector_tools,
            mail_actions: None,
        }
    }

    pub fn with_automation_tools(
        mut self,
        automation_tools: Option<agent_runtime::automation_tools::AutomationToolRuntime>,
    ) -> Self {
        self.automation_tools = automation_tools;
        self
    }

    pub fn with_attachment_tools(
        mut self,
        attachment_tools: Option<agent_runtime::attachment_tools::AttachmentToolRuntime>,
    ) -> Self {
        self.attachment_tools = attachment_tools;
        self
    }

    pub fn with_mail_actions(
        mut self,
        mail_actions: Option<agent_runtime::foundation_actions::MailActionService>,
    ) -> Self {
        self.mail_actions = mail_actions;
        self
    }
}
