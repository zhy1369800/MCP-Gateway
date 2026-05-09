impl SkillsService {
    #[cfg(test)]
    async fn create_confirmation(
        &self,
        skill: &str,
        display_name: &str,
        args: &[String],
        raw_command: &str,
        reason: &str,
    ) -> CreateConfirmationResult {
        self.create_confirmation_with_metadata(
            skill,
            display_name,
            args,
            raw_command,
            reason,
            ConfirmationMetadata {
                kind: "skill".to_string(),
                cwd: String::new(),
                affected_paths: Vec::new(),
                preview: raw_command.to_string(),
                reason_key: String::new(),
            },
        )
        .await
    }

    async fn create_confirmation_with_metadata(
        &self,
        skill: &str,
        display_name: &str,
        args: &[String],
        raw_command: &str,
        reason: &str,
        metadata: ConfirmationMetadata,
    ) -> CreateConfirmationResult {
        let fingerprint = format!("{skill}|{raw_command}");
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);

        // 检查同指纹是否已有条目：
        // - Pending  → 复用，不重复弹窗
        // - 刚超时的 Rejected (timed_out=true) → 直接告知调用方已超时，不新建
        // - 用户手动 Rejected / Approved → 允许重新发起
        for entry in guard.values() {
            if entry.fingerprint != fingerprint {
                continue;
            }
            match entry.record.status {
                ConfirmationStatus::Pending => {
                    return CreateConfirmationResult::Reused(entry.record.clone());
                }
                ConfirmationStatus::Rejected if entry.timed_out => {
                    return CreateConfirmationResult::AlreadyTimedOut(entry.record.id.clone());
                }
                _ => {}
            }
        }

        let id = Uuid::new_v4().to_string();
        let record = SkillConfirmation {
            id: id.clone(),
            status: ConfirmationStatus::Pending,
            created_at: now,
            updated_at: now,
            kind: metadata.kind,
            skill: skill.to_string(),
            display_name: display_name.to_string(),
            args: args.to_vec(),
            raw_command: raw_command.to_string(),
            cwd: metadata.cwd,
            affected_paths: metadata.affected_paths,
            preview: metadata.preview,
            reason: reason.to_string(),
            reason_key: metadata.reason_key,
        };

        guard.insert(
            id,
            ConfirmationEntry {
                fingerprint,
                record: record.clone(),
                notify: Arc::new(Notify::new()),
                timed_out: false,
            },
        );
        let timeout_service = self.clone();
        let timeout_id = record.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Self::CONFIRMATION_DECISION_TIMEOUT).await;
            timeout_service
                .reject_confirmation_on_timeout(&timeout_id)
                .await;
        });
        CreateConfirmationResult::Created(record)
    }

    async fn wait_for_confirmation_decision(
        &self,
        confirmation_id: &str,
        timeout: Duration,
        _poll_interval: Duration,
    ) -> ConfirmationWaitOutcome {
        let started = Instant::now();
        loop {
            let wait_notify = {
                let now = Utc::now();
                let mut guard = self.confirmations.write().await;
                Self::prune_confirmations_locked(&mut guard, now);
                match guard.get(confirmation_id).map(|entry| {
                    (
                        entry.record.status.clone(),
                        entry.record.created_at,
                        entry.notify.clone(),
                        entry.timed_out,
                    )
                }) {
                    Some((ConfirmationStatus::Approved, _, _, _)) => {
                        guard.remove(confirmation_id);
                        return ConfirmationWaitOutcome::Approved;
                    }
                    Some((ConfirmationStatus::Rejected, _, _, timed_out)) => {
                        guard.remove(confirmation_id);
                        return if timed_out {
                            ConfirmationWaitOutcome::TimedOut
                        } else {
                            ConfirmationWaitOutcome::Rejected
                        };
                    }
                    Some((ConfirmationStatus::Pending, created_at, notify, _)) => {
                        if Self::age_exceeds(created_at, now, timeout) {
                            if let Some(entry) = guard.get_mut(confirmation_id) {
                                entry.record.status = ConfirmationStatus::Rejected;
                                entry.record.updated_at = now;
                                entry.timed_out = true;
                                entry.notify.notify_one();
                            }
                            return ConfirmationWaitOutcome::TimedOut;
                        }
                        notify
                    }
                    None => return ConfirmationWaitOutcome::TimedOut,
                }
            };

            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                let mut guard = self.confirmations.write().await;
                if let Some(entry) = guard.get_mut(confirmation_id) {
                    entry.record.status = ConfirmationStatus::Rejected;
                    entry.record.updated_at = Utc::now();
                    entry.timed_out = true;
                    entry.notify.notify_one();
                }
                return ConfirmationWaitOutcome::TimedOut;
            };
            if remaining.is_zero() {
                let mut guard = self.confirmations.write().await;
                if let Some(entry) = guard.get_mut(confirmation_id) {
                    entry.record.status = ConfirmationStatus::Rejected;
                    entry.record.updated_at = Utc::now();
                    entry.timed_out = true;
                    entry.notify.notify_one();
                }
                return ConfirmationWaitOutcome::TimedOut;
            }

            let notified = tokio::time::timeout(remaining, wait_notify.notified()).await;
            if notified.is_err() {
                let mut guard = self.confirmations.write().await;
                if let Some(entry) = guard.get_mut(confirmation_id) {
                    entry.record.status = ConfirmationStatus::Rejected;
                    entry.record.updated_at = Utc::now();
                    entry.timed_out = true;
                    entry.notify.notify_one();
                }
                return ConfirmationWaitOutcome::TimedOut;
            }
        }
    }

    async fn reject_confirmation_on_timeout(&self, id: &str) {
        let now = Utc::now();
        let mut guard = self.confirmations.write().await;
        Self::prune_confirmations_locked(&mut guard, now);
        let Some(entry) = guard.get_mut(id) else {
            return;
        };
        if entry.record.status != ConfirmationStatus::Pending {
            return;
        }
        entry.record.status = ConfirmationStatus::Rejected;
        entry.record.updated_at = now;
        entry.timed_out = true;
        entry.notify.notify_one();
    }

    fn age_exceeds(created_at: DateTime<Utc>, now: DateTime<Utc>, ttl: Duration) -> bool {
        now.signed_duration_since(created_at)
            .to_std()
            .map(|elapsed| elapsed >= ttl)
            .unwrap_or(false)
    }

    fn is_same_confirmation_signature(left: &SkillConfirmation, right: &SkillConfirmation) -> bool {
        left.skill == right.skill
            && left.display_name == right.display_name
            && left.args == right.args
            && left.raw_command == right.raw_command
            && left.reason == right.reason
    }

    fn prune_confirmations_locked(
        confirmations: &mut HashMap<String, ConfirmationEntry>,
        now: DateTime<Utc>,
    ) {
        confirmations.retain(|_, entry| match entry.record.status {
            ConfirmationStatus::Pending => !Self::age_exceeds(
                entry.record.created_at,
                now,
                Self::CONFIRMATION_STALE_PENDING_WINDOW,
            ),
            ConfirmationStatus::Approved | ConfirmationStatus::Rejected => !Self::age_exceeds(
                entry.record.updated_at,
                now,
                Self::CONFIRMATION_RESOLVED_RETENTION_WINDOW,
            ),
        });
    }

}
