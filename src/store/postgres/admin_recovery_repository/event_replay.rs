use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::domain::workflow_instance::recovery::{
    BeforeSnapshotV1, RecoveryError, WorkflowProjection,
};

use super::event_fields::{
    admin_payload_is_bounded, event_data, exact_keys, optional_string_field, string_field,
    uuid_field,
};
use super::import_event;
use super::rows::{ContextFact, EventFact, InstanceRow, SubmissionFact, TransitionFact, VisitFact};

fn invalid(detail: impl Into<String>) -> RecoveryError {
    RecoveryError::InvalidImmutableFacts(detail.into())
}

struct Replay<'a> {
    instance: &'a InstanceRow,
    definition_digest: Option<&'a str>,
    contexts: HashMap<Uuid, &'a ContextFact>,
    visits: HashMap<Uuid, &'a VisitFact>,
    submissions: HashMap<Uuid, &'a SubmissionFact>,
    transitions: HashMap<Uuid, &'a TransitionFact>,
    introduced_contexts: HashSet<Uuid>,
    introduced_visits: HashSet<Uuid>,
    introduced_submissions: HashSet<Uuid>,
    visit_counts: HashMap<Uuid, i32>,
    current_context: Option<Uuid>,
    current_visit: Option<Uuid>,
    version: i32,
}

impl<'a> Replay<'a> {
    fn new(
        instance: &'a InstanceRow,
        definition_digest: Option<&'a str>,
        contexts: &'a [ContextFact],
        visits: &'a [VisitFact],
        submissions: &'a [SubmissionFact],
        transitions: &'a [TransitionFact],
    ) -> Self {
        Self {
            instance,
            definition_digest,
            contexts: contexts
                .iter()
                .map(|fact| (fact.context_revision_id, fact))
                .collect(),
            visits: visits
                .iter()
                .map(|fact| (fact.node_visit_id, fact))
                .collect(),
            submissions: submissions
                .iter()
                .map(|fact| (fact.submission_id, fact))
                .collect(),
            transitions: transitions
                .iter()
                .map(|fact| (fact.transition_id, fact))
                .collect(),
            introduced_contexts: HashSet::new(),
            introduced_visits: HashSet::new(),
            introduced_submissions: HashSet::new(),
            visit_counts: HashMap::new(),
            current_context: None,
            current_visit: None,
            version: 0,
        }
    }

    fn visit(&self, id: Option<Uuid>) -> Result<&'a VisitFact, RecoveryError> {
        id.and_then(|value| self.visits.get(&value).copied())
            .ok_or_else(|| invalid("event references an invalid node visit"))
    }

    fn context(&self, id: Option<Uuid>) -> Result<&'a ContextFact, RecoveryError> {
        id.and_then(|value| self.contexts.get(&value).copied())
            .ok_or_else(|| invalid("event references an invalid context revision"))
    }

    fn introduce_visit(&mut self, visit: &VisitFact) -> Result<(), RecoveryError> {
        if !self.introduced_visits.insert(visit.node_visit_id) {
            return Err(invalid("node visit is introduced more than once"));
        }
        let count = self.visit_counts.entry(visit.node_id).or_insert(0);
        *count = count
            .checked_add(1)
            .ok_or_else(|| invalid("node visit count overflow"))?;
        if visit.visit_number != *count {
            return Err(invalid("node visit number does not follow event order"));
        }
        Ok(())
    }

    fn introduce_context(&mut self, context: &ContextFact) -> Result<(), RecoveryError> {
        if !self.introduced_contexts.insert(context.context_revision_id) {
            return Err(invalid("context revision is introduced more than once"));
        }
        Ok(())
    }

    fn introduce_submission(&mut self, submission: &SubmissionFact) -> Result<(), RecoveryError> {
        if !self.introduced_submissions.insert(submission.submission_id) {
            return Err(invalid("submission is introduced more than once"));
        }
        Ok(())
    }

    fn validate_node_columns(
        &self,
        event: &EventFact,
        source: Option<&VisitFact>,
        target: Option<&VisitFact>,
    ) -> Result<(), RecoveryError> {
        if event.from_node_id != source.map(|visit| visit.node_id)
            || event.to_node_id != target.map(|visit| visit.node_id)
        {
            return Err(invalid("event node fields disagree with visit references"));
        }
        Ok(())
    }

    fn apply_initial(&mut self, event: &EventFact) -> Result<(), RecoveryError> {
        let data = event_data(event)?;
        let imported = event.event_type == "WORKFLOW_INSTANCE_IMPORTED";
        let keys = if imported {
            [
                "legacySystem",
                "legacyRecordId",
                "legacySnapshotDigest",
                "importedNodeId",
                "importedAt",
                "creatorResolution",
            ]
            .as_slice()
        } else {
            [
                "definition_version_id",
                "definition_digest",
                "initial_node_id",
                "assignee_resolution_type",
            ]
            .as_slice()
        };
        let context = self.context(event.context_revision_id)?;
        let visit = self.visit(event.target_node_visit_id)?;
        if self.version != 0
            || !exact_keys(data, keys)
            || event.source_node_visit_id.is_some()
            || event.submission_id.is_some()
            || event.transition_effect.is_some()
            || event.from_node_id.is_some()
            || event.to_node_id.is_some()
            || context.revision_number != 1
            || context.previous_revision_id.is_some()
            || visit.entered_by_transition_id.is_some()
            || visit.visit_number != 1
        {
            return Err(invalid(
                "initial event does not introduce revision 1 and visit 1",
            ));
        }
        if imported {
            import_event::validate(data, event, self.instance, context, visit)?;
        } else if uuid_field(data, "definition_version_id")
            != Some(self.instance.definition_version_id)
            || string_field(data, "definition_digest") != Some(self.definition_digest.unwrap_or(""))
            || uuid_field(data, "initial_node_id") != Some(visit.node_id)
            || string_field(data, "assignee_resolution_type") != visit.assignee_ref_type.as_deref()
            || visit.node_type != "DRAFT"
            || visit.assignee_principal_id.is_none()
        {
            return Err(invalid(
                "creation event data does not match the initial facts",
            ));
        }
        self.introduce_context(context)?;
        self.introduce_visit(visit)?;
        self.current_context = Some(context.context_revision_id);
        self.current_visit = Some(visit.node_visit_id);
        Ok(())
    }

    fn apply_context_revision(&mut self, event: &EventFact) -> Result<(), RecoveryError> {
        let data = event_data(event)?;
        let context = self.context(event.context_revision_id)?;
        let visit = self.visit(event.source_node_visit_id)?;
        let previous = self.context(self.current_context)?;
        if !exact_keys(
            data,
            &[
                "previous_context_revision_id",
                "new_context_revision_id",
                "previous_payload_digest",
                "new_payload_digest",
                "current_node_id",
            ],
        ) || event.source_node_visit_id != self.current_visit
            || event.target_node_visit_id != self.current_visit
            || event.submission_id.is_some()
            || event.transition_effect.is_some()
            || event.from_node_id.is_some()
            || event.to_node_id.is_some()
            || context.previous_revision_id != self.current_context
            || context.revision_number != previous.revision_number + 1
            || uuid_field(data, "previous_context_revision_id") != self.current_context
            || uuid_field(data, "new_context_revision_id") != Some(context.context_revision_id)
            || string_field(data, "previous_payload_digest")
                != Some(previous.payload_digest.as_str())
            || string_field(data, "new_payload_digest") != Some(context.payload_digest.as_str())
            || uuid_field(data, "current_node_id") != Some(visit.node_id)
            || visit.node_type != "DRAFT"
        {
            return Err(invalid(
                "context revision event disagrees with replay state",
            ));
        }
        self.introduce_context(context)?;
        self.current_context = Some(context.context_revision_id);
        Ok(())
    }

    fn transition_and_visits(
        &self,
        event: &EventFact,
        data: &serde_json::Value,
    ) -> Result<(&'a VisitFact, &'a VisitFact, &'a TransitionFact), RecoveryError> {
        let source = self.visit(event.source_node_visit_id)?;
        let target = self.visit(event.target_node_visit_id)?;
        let transition_id = uuid_field(data, "transition_definition_id")
            .ok_or_else(|| invalid("transition event has no valid transition id"))?;
        let transition = self
            .transitions
            .get(&transition_id)
            .copied()
            .ok_or_else(|| invalid("transition event references an invalid definition"))?;
        if event.source_node_visit_id != self.current_visit
            || self.introduced_visits.contains(&target.node_visit_id)
            || target.entered_by_transition_id != Some(transition.transition_id)
            || transition.source_node_id != source.node_id
            || transition.target_node_id != target.node_id
            || event.transition_effect.as_deref() != Some(transition.transition_effect.as_str())
            || string_field(data, "transition_key") != Some(transition.transition_key.as_str())
            || string_field(data, "transition_effect")
                != Some(transition.transition_effect.as_str())
            || uuid_field(data, "source_node_id") != Some(source.node_id)
            || uuid_field(data, "target_node_id") != Some(target.node_id)
            || uuid_field(data, "source_node_visit_id") != Some(source.node_visit_id)
            || uuid_field(data, "target_node_visit_id") != Some(target.node_visit_id)
        {
            return Err(invalid(
                "transition event disagrees with definition or replay state",
            ));
        }
        self.validate_node_columns(event, Some(source), Some(target))?;
        Ok((source, target, transition))
    }

    fn apply_transition(&mut self, event: &EventFact) -> Result<(), RecoveryError> {
        let data = event_data(event)?;
        if !exact_keys(
            data,
            &[
                "transition_definition_id",
                "transition_key",
                "transition_effect",
                "source_node_id",
                "target_node_id",
                "source_node_visit_id",
                "target_node_visit_id",
                "context_revision_id",
                "submission_payload_digest",
            ],
        ) || event.context_revision_id != self.current_context
            || uuid_field(data, "context_revision_id") != self.current_context
        {
            return Err(invalid("transition event data shape is invalid"));
        }
        let (_, target, transition) = self.transition_and_visits(event, data)?;
        match event.submission_id {
            Some(id) => {
                let submission =
                    self.submissions.get(&id).copied().ok_or_else(|| {
                        invalid("transition event references an invalid submission")
                    })?;
                if submission.source_node_visit_id != event.source_node_visit_id.unwrap()
                    || Some(submission.context_revision_id) != self.current_context
                    || submission.transition_id != transition.transition_id
                    || optional_string_field(data, "submission_payload_digest")
                        != Some(Some(submission.payload_digest.as_str()))
                {
                    return Err(invalid("transition submission disagrees with event"));
                }
                self.introduce_submission(submission)?;
            }
            None if optional_string_field(data, "submission_payload_digest") != Some(None) => {
                return Err(invalid(
                    "transition event omits a referenced submission digest",
                ));
            }
            None => {}
        }
        self.introduce_visit(target)?;
        self.current_visit = Some(target.node_visit_id);
        Ok(())
    }

    fn apply_combined(&mut self, event: &EventFact) -> Result<(), RecoveryError> {
        let data = event_data(event)?;
        let keys = [
            "previous_context_revision_id",
            "new_context_revision_id",
            "previous_context_payload_digest",
            "new_context_payload_digest",
            "transition_definition_id",
            "transition_key",
            "transition_effect",
            "source_node_id",
            "target_node_id",
            "source_node_visit_id",
            "target_node_visit_id",
            "submission_payload_digest",
        ];
        let previous = self.context(self.current_context)?;
        let context = self.context(event.context_revision_id)?;
        if !exact_keys(data, &keys)
            || event.transition_effect.as_deref() != Some("ADVANCE")
            || context.previous_revision_id != self.current_context
            || context.revision_number != previous.revision_number + 1
            || uuid_field(data, "previous_context_revision_id") != self.current_context
            || uuid_field(data, "new_context_revision_id") != Some(context.context_revision_id)
            || string_field(data, "previous_context_payload_digest")
                != Some(previous.payload_digest.as_str())
            || string_field(data, "new_context_payload_digest")
                != Some(context.payload_digest.as_str())
        {
            return Err(invalid(
                "combined event context disagrees with replay state",
            ));
        }
        let (_, target, transition) = self.transition_and_visits(event, data)?;
        let submission = event
            .submission_id
            .and_then(|id| self.submissions.get(&id).copied())
            .ok_or_else(|| invalid("combined event requires a submission"))?;
        if submission.source_node_visit_id != event.source_node_visit_id.unwrap()
            || submission.context_revision_id != context.context_revision_id
            || submission.transition_id != transition.transition_id
            || string_field(data, "submission_payload_digest")
                != Some(submission.payload_digest.as_str())
        {
            return Err(invalid("combined submission disagrees with event"));
        }
        self.introduce_context(context)?;
        self.introduce_submission(submission)?;
        self.introduce_visit(target)?;
        self.current_context = Some(context.context_revision_id);
        self.current_visit = Some(target.node_visit_id);
        Ok(())
    }

    fn apply_admin(&mut self, event: &EventFact) -> Result<(), RecoveryError> {
        let data = event_data(event)?;
        let source = self.visit(event.source_node_visit_id)?;
        let target = self.visit(event.target_node_visit_id)?;
        let operation = string_field(data, "operation");
        let expected_effect = match operation {
            Some("MOVE_TO_NODE") if target.node_type != "TERMINAL" => "ADVANCE",
            Some("TERMINATE_INSTANCE") if target.node_type == "TERMINAL" => "TERMINATE",
            _ => return Err(invalid("admin operation does not match target node type")),
        };
        let before = BeforeSnapshotV1::new(
            self.instance.workflow_instance_id,
            self.instance.domain_id,
            self.instance.definition_version_id,
            self.instance.created_by_principal_id,
            &WorkflowProjection {
                current_context_revision_id: self.current_context,
                current_node_visit_id: self.current_visit,
                workflow_state_version: self.version,
            },
        )
        .digest()?;
        if !exact_keys(
            data,
            &[
                "operation",
                "reason",
                "relatedReferences",
                "beforeSnapshotDigest",
            ],
        ) || event.source_node_visit_id != self.current_visit
            || event.context_revision_id != self.current_context
            || event.submission_id.is_some()
            || self.introduced_visits.contains(&target.node_visit_id)
            || target.entered_by_transition_id.is_some()
            || event.transition_effect.as_deref() != Some(expected_effect)
            || !admin_payload_is_bounded(data)
            || string_field(data, "beforeSnapshotDigest") != Some(before.as_str())
        {
            return Err(invalid("admin event disagrees with replay state"));
        }
        self.validate_node_columns(event, Some(source), Some(target))?;
        self.introduce_visit(target)?;
        self.current_visit = Some(target.node_visit_id);
        Ok(())
    }

    fn apply(&mut self, event: &EventFact, expected_sequence: i32) -> Result<(), RecoveryError> {
        if event.workflow_instance_id != self.instance.workflow_instance_id
            || event.event_sequence != expected_sequence
            || event.event_schema_version != "v1"
            || event.old_workflow_state_version != self.version
            || event.new_workflow_state_version != expected_sequence
        {
            return Err(invalid(
                "event sequence and state versions are not contiguous",
            ));
        }
        match event.event_type.as_str() {
            "INSTANCE_CREATED" | "WORKFLOW_INSTANCE_CREATED" | "WORKFLOW_INSTANCE_IMPORTED" => {
                self.apply_initial(event)?
            }
            "CONTEXT_REVISED" | "WORKFLOW_CONTEXT_REVISED" => self.apply_context_revision(event)?,
            "WORKFLOW_TRANSITION_COMMITTED" => self.apply_transition(event)?,
            "WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED" => self.apply_combined(event)?,
            "ADMIN_EMERGENCY_OVERRIDE_COMMITTED" => self.apply_admin(event)?,
            _ => return Err(invalid("event type is not supported by recovery replay")),
        }
        self.version = expected_sequence;
        Ok(())
    }

    fn finish(self) -> Result<WorkflowProjection, RecoveryError> {
        if self.introduced_contexts.len() != self.contexts.len()
            || self.introduced_visits.len() != self.visits.len()
            || self.introduced_submissions.len() != self.submissions.len()
        {
            return Err(invalid(
                "immutable fact exists outside its introducing event",
            ));
        }
        Ok(WorkflowProjection {
            current_context_revision_id: self.current_context,
            current_node_visit_id: self.current_visit,
            workflow_state_version: self.version,
        })
    }
}

pub(super) fn replay(
    instance: &InstanceRow,
    definition_digest: Option<&str>,
    contexts: &[ContextFact],
    visits: &[VisitFact],
    submissions: &[SubmissionFact],
    transitions: &[TransitionFact],
    events: &[EventFact],
) -> Result<WorkflowProjection, RecoveryError> {
    if events.is_empty() {
        return Err(invalid("event fact sequence is empty"));
    }
    let mut replay = Replay::new(
        instance,
        definition_digest,
        contexts,
        visits,
        submissions,
        transitions,
    );
    for (index, event) in events.iter().enumerate() {
        let expected = i32::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| invalid("event sequence overflows i32"))?;
        replay.apply(event, expected)?;
    }
    replay.finish()
}
