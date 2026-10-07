use super::{
    ActivityProjection, ActivityStage, ClaimedWork, SupervisorError, SupervisorWorkStatus,
    WorkspaceSupervisor,
};

/// The narrow Adapter seam from Workspace supervision into a concrete worker
/// runtime. Implementations may use the current delegated runtime or a later
/// durable work-package scheduler; neither leaks into callers of the Actor.
pub trait WorkDispatcher {
    fn dispatch(&self, work: &ClaimedWork) -> Result<DispatchedWork, String>;
}

/// Structured result from one dispatch. It deliberately contains only the
/// Workspace-visible activity needed to continue the control-plane state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchedWork {
    pub work_package_id: String,
    pub objective: String,
    pub stage: ActivityStage,
    pub summary: String,
    pub artifact_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchOutcome {
    pub input_id: String,
    pub work_package_id: String,
}

/// Executes one safe scheduling unit for one Workspace. It contains no model,
/// filesystem, or tool authority; those are owned by the injected Adapter.
pub struct WorkspaceSupervisorActor<D> {
    supervisor: WorkspaceSupervisor,
    dispatcher: D,
}

impl<D> WorkspaceSupervisorActor<D>
where
    D: WorkDispatcher,
{
    pub fn new(supervisor: WorkspaceSupervisor, dispatcher: D) -> Self {
        Self {
            supervisor,
            dispatcher,
        }
    }

    pub fn dispatch_next(
        &self,
        workspace_id: &str,
        now: i64,
    ) -> Result<Option<DispatchOutcome>, SupervisorError> {
        let Some(claim) = self.supervisor.claim_next_work(workspace_id, now)? else {
            return Ok(None);
        };
        let dispatched = match self.dispatcher.dispatch(&claim) {
            Ok(dispatched) => dispatched,
            Err(error) => {
                self.supervisor.release_work(&claim, now)?;
                return Err(SupervisorError::Dispatch(error));
            }
        };
        let projection = ActivityProjection {
            workspace_id: claim.work.workspace_id.clone(),
            work_package_id: dispatched.work_package_id.clone(),
            objective: dispatched.objective,
            stage: dispatched.stage,
            summary: dispatched.summary,
            artifact_refs: dispatched.artifact_refs,
            updated_at: now,
        };
        self.supervisor.publish_activity(projection)?;
        self.supervisor
            .settle_work(&claim, SupervisorWorkStatus::Completed, now)?;
        Ok(Some(DispatchOutcome {
            input_id: claim.work.id,
            work_package_id: dispatched.work_package_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::{agent::shared_db::SharedDb, db::migrate};

    struct FakeDispatcher {
        fail: bool,
    }

    impl WorkDispatcher for FakeDispatcher {
        fn dispatch(&self, work: &ClaimedWork) -> Result<DispatchedWork, String> {
            if self.fail {
                return Err("temporary dispatch failure".into());
            }
            Ok(DispatchedWork {
                work_package_id: format!("package-{}", work.work.id),
                objective: work.work.objective.clone(),
                stage: work.work.kind.initial_stage(),
                summary: "Main Agent accepted the work.".into(),
                artifact_refs: vec![],
            })
        }
    }

    fn supervisor() -> WorkspaceSupervisor {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        connection.execute("INSERT INTO projects (id, name, path, created_at, kind, updated_at) VALUES ('p', 'p', 'C:/p', 1, 'project', 1)", []).unwrap();
        WorkspaceSupervisor::new(SharedDb::new(connection))
    }

    #[test]
    fn dispatches_once_then_projects_the_workspace_work() {
        let supervisor = supervisor();
        let input = supervisor
            .submit(
                "p",
                super::super::SupervisorInputKind::UserTask,
                "inspect project",
                1,
            )
            .unwrap();
        supervisor
            .queue_work(
                "p",
                Some(input.id),
                super::super::SupervisorWorkKind::Explore,
                "inspect project",
                1,
            )
            .unwrap();
        let actor =
            WorkspaceSupervisorActor::new(supervisor.clone(), FakeDispatcher { fail: false });
        let outcome = actor.dispatch_next("p", 2).unwrap().unwrap();
        assert!(outcome.work_package_id.starts_with("package-wswork_"));
        assert_eq!(
            supervisor.activities("p").unwrap()[0].objective,
            "inspect project"
        );
        assert!(actor.dispatch_next("p", 3).unwrap().is_none());
    }

    #[test]
    fn failed_dispatch_releases_the_input_for_a_later_attempt() {
        let supervisor = supervisor();
        supervisor
            .queue_work(
                "p",
                None,
                super::super::SupervisorWorkKind::Explore,
                "inspect project",
                1,
            )
            .unwrap();
        let failing =
            WorkspaceSupervisorActor::new(supervisor.clone(), FakeDispatcher { fail: true });
        assert!(matches!(
            failing.dispatch_next("p", 2),
            Err(SupervisorError::Dispatch(_))
        ));
        let succeeding = WorkspaceSupervisorActor::new(supervisor, FakeDispatcher { fail: false });
        assert!(succeeding.dispatch_next("p", 3).unwrap().is_some());
    }
}
