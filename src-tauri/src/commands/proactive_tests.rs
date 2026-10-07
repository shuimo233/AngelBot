//! Proactive background ability tests — Issue #057
//!
//! Tests: scheduled task recovery, notification channel toggle, settings persistence.

use crate::agent::event::EventChannel;
use crate::commands::settings::{NotificationSettings, ScheduledTask};
use serde_json::json;

#[cfg(test)]
mod tests {
    use super::*;

    // =====================================================================
    // Notification Channel Tests
    // =====================================================================

    #[test]
    fn test_notification_settings_default_all_enabled() {
        let settings = NotificationSettings::default();
        assert!(
            settings.chat_reply,
            "chat_reply should be enabled by default"
        );
        assert!(settings.reminder, "reminder should be enabled by default");
        assert!(
            settings.task_complete,
            "task_complete should be enabled by default"
        );
        assert!(
            settings.file_change,
            "file_change should be enabled by default"
        );
        assert!(settings.system, "system should be enabled by default");
    }

    #[test]
    fn test_notification_settings_disable_chat_reply() {
        let mut settings = NotificationSettings::default();
        settings.chat_reply = false;

        assert!(!settings.chat_reply);
        // Other channels remain enabled
        assert!(settings.reminder);
        assert!(settings.task_complete);
    }

    #[test]
    fn test_notification_settings_disable_reminder() {
        let mut settings = NotificationSettings::default();
        settings.reminder = false;

        assert!(!settings.reminder);
        assert!(settings.chat_reply);
    }

    #[test]
    fn test_notification_settings_disable_file_change() {
        let mut settings = NotificationSettings::default();
        settings.file_change = false;

        assert!(!settings.file_change);
        assert!(settings.system);
    }

    #[test]
    fn test_notification_settings_serialization_roundtrip() {
        let mut settings = NotificationSettings::default();
        settings.chat_reply = false;
        settings.reminder = true;
        settings.task_complete = false;
        settings.file_change = true;
        settings.system = false;

        let json_str = serde_json::to_string(&settings).unwrap();
        let restored: NotificationSettings = serde_json::from_str(&json_str).unwrap();

        assert!(!restored.chat_reply);
        assert!(restored.reminder);
        assert!(!restored.task_complete);
        assert!(restored.file_change);
        assert!(!restored.system);
    }

    #[test]
    fn test_notification_settings_all_disabled() {
        let mut settings = NotificationSettings::default();
        settings.chat_reply = false;
        settings.reminder = false;
        settings.task_complete = false;
        settings.file_change = false;
        settings.system = false;

        let json_str = serde_json::to_string(&settings).unwrap();
        let restored: NotificationSettings = serde_json::from_str(&json_str).unwrap();

        assert!(!restored.chat_reply);
        assert!(!restored.reminder);
        assert!(!restored.task_complete);
        assert!(!restored.file_change);
        assert!(!restored.system);
    }

    // =====================================================================
    // Scheduled Task Tests
    // =====================================================================

    #[test]
    fn test_scheduled_task_enabled() {
        let task = ScheduledTask {
            id: "test-1".to_string(),
            task_type: "scheduled_reminder".to_string(),
            trigger_at: "2025-01-01T12:00:00Z".to_string(),
            content: "Test task".to_string(),
            enabled: true,
        };

        assert!(task.enabled);
        assert_eq!(task.task_type, "scheduled_reminder");
        assert_eq!(task.id, "test-1");
    }

    #[test]
    fn test_scheduled_task_disabled() {
        let task = ScheduledTask {
            id: "test-2".to_string(),
            task_type: "scheduled_reminder".to_string(),
            trigger_at: "2025-01-01T12:00:00Z".to_string(),
            content: "Disabled task".to_string(),
            enabled: false,
        };

        assert!(!task.enabled);
    }

    #[test]
    fn test_scheduled_task_future_trigger() {
        let future_time = "2099-12-31T23:59:59Z";
        let task = ScheduledTask {
            id: "future-1".to_string(),
            task_type: "scheduled_reminder".to_string(),
            trigger_at: future_time.to_string(),
            content: "Future reminder".to_string(),
            enabled: true,
        };

        assert_eq!(task.trigger_at, future_time);
        assert!(task.enabled);
    }

    #[test]
    fn test_scheduled_task_past_trigger() {
        let task = ScheduledTask {
            id: "past-1".to_string(),
            task_type: "scheduled_reminder".to_string(),
            trigger_at: "2020-01-01T00:00:00Z".to_string(),
            content: "Past reminder".to_string(),
            enabled: true,
        };

        assert!(!task.trigger_at.is_empty());
    }

    #[test]
    fn test_scheduled_task_serialization_roundtrip() {
        let task = ScheduledTask {
            id: "serial-test".to_string(),
            task_type: "scheduled_reminder".to_string(),
            trigger_at: "2025-07-15T10:30:00Z".to_string(),
            content: "Serialization test".to_string(),
            enabled: true,
        };

        let json_str = serde_json::to_string(&task).unwrap();
        let restored: ScheduledTask = serde_json::from_str(&json_str).unwrap();

        assert_eq!(restored.id, "serial-test");
        assert_eq!(restored.content, "Serialization test");
        assert!(restored.enabled);
        assert_eq!(restored.trigger_at, "2025-07-15T10:30:00Z");
    }

    #[test]
    fn test_scheduled_task_multiple_types() {
        let reminder_task = ScheduledTask {
            id: "r1".to_string(),
            task_type: "scheduled_reminder".to_string(),
            trigger_at: "2025-01-01T12:00:00Z".to_string(),
            content: "Reminder".to_string(),
            enabled: true,
        };

        let file_task = ScheduledTask {
            id: "f1".to_string(),
            task_type: "file_watcher".to_string(),
            trigger_at: "2025-01-01T12:00:00Z".to_string(),
            content: "File change watch".to_string(),
            enabled: true,
        };

        assert_eq!(reminder_task.task_type, "scheduled_reminder");
        assert_eq!(file_task.task_type, "file_watcher");
    }

    #[test]
    fn test_scheduled_task_enabled_filter() {
        let tasks = vec![
            ScheduledTask {
                id: "t1".to_string(),
                task_type: "scheduled_reminder".to_string(),
                trigger_at: "2025-01-01T12:00:00Z".to_string(),
                content: "Enabled task".to_string(),
                enabled: true,
            },
            ScheduledTask {
                id: "t2".to_string(),
                task_type: "scheduled_reminder".to_string(),
                trigger_at: "2025-01-01T13:00:00Z".to_string(),
                content: "Disabled task".to_string(),
                enabled: false,
            },
            ScheduledTask {
                id: "t3".to_string(),
                task_type: "scheduled_reminder".to_string(),
                trigger_at: "2025-01-01T14:00:00Z".to_string(),
                content: "Another enabled".to_string(),
                enabled: true,
            },
        ];

        let enabled: Vec<_> = tasks.iter().filter(|t| t.enabled).collect();
        let disabled: Vec<_> = tasks.iter().filter(|t| !t.enabled).collect();

        assert_eq!(enabled.len(), 2);
        assert_eq!(disabled.len(), 1);
    }

    // =====================================================================
    // Event Channel Tests (Agent Inbox)
    // =====================================================================

    #[test]
    fn test_event_channel_stores_agent_start_event() {
        let channel = EventChannel::new();

        channel.emit(crate::agent::event::AgentEvent::AgentStart {
            session_id: "scheduled-session".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
        });

        let events = channel.get_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            crate::agent::event::AgentEvent::AgentStart { .. }
        ));
    }

    #[test]
    fn test_event_channel_clears_events() {
        let channel = EventChannel::new();

        channel.emit(crate::agent::event::AgentEvent::AgentStart {
            session_id: "test".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
        });
        assert_eq!(channel.get_events().len(), 1);

        channel.clear();
        assert_eq!(channel.get_events().len(), 0);
    }

    #[test]
    fn test_event_channel_multiple_events() {
        let channel = EventChannel::new();

        for i in 0..5 {
            channel.emit(crate::agent::event::AgentEvent::MessageStart {
                turn_id: format!("turn-{}", i),
                message_id: format!("msg-{}", i),
            });
        }

        assert_eq!(channel.get_events().len(), 5);
    }

    #[test]
    fn test_event_channel_stores_scheduled_reminder_event() {
        let channel = EventChannel::new();

        // Simulate a scheduled reminder event being injected into the agent inbox
        channel.emit(crate::agent::event::AgentEvent::TurnStart {
            turn_id: "scheduled-reminder-1".to_string(),
            session_id: "background-session".to_string(),
            message: "Scheduled reminder: check email".to_string(),
        });

        let events = channel.get_events();
        assert_eq!(events.len(), 1);

        if let crate::agent::event::AgentEvent::TurnStart { message, .. } = &events[0] {
            assert!(message.contains("Scheduled reminder"));
        } else {
            panic!("Expected TurnStart event");
        }
    }

    // =====================================================================
    // Settings Persistence Smoke Tests
    // =====================================================================

    #[test]
    fn test_notification_settings_smoke() {
        let mut settings = NotificationSettings::default();
        settings.chat_reply = false;
        settings.system = false;

        let json = serde_json::to_string(&settings).unwrap();
        let restored: NotificationSettings = serde_json::from_str(&json).unwrap();

        assert!(!restored.chat_reply);
        assert!(restored.reminder);
        assert!(!restored.system);
    }

    #[test]
    fn test_scheduled_task_smoke() {
        let task = ScheduledTask {
            id: "smoke-test".to_string(),
            task_type: "scheduled_reminder".to_string(),
            trigger_at: "2025-07-15T10:30:00Z".to_string(),
            content: "Smoke test task".to_string(),
            enabled: true,
        };

        let json = serde_json::to_string(&task).unwrap();
        let restored: ScheduledTask = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, "smoke-test");
        assert!(restored.enabled);
    }
}
