use super::*;

pub(super) fn run(
    this: &SuppliedTool,
    arguments: &serde_json::Value,
    ops: &dyn Ops,
    cancel: &dyn Cancel,
) -> Output {
    let mut call = Call {
        tool: this.card.name.clone(),
        args: arguments.clone(),
        cwd: ops.cwd().display().to_string(),
        answered: None,
        revision: Default::default(),
    };
    // magi decides, the program describes: the card says which verb this tool acts under and
    // the ledger answers, so a tool moved out of magi's config cannot leave the gate behind it.
    let action = if let Some(capability) = &this.card.file_revision {
        if capability.version != 1 {
            return Output::error("native file capability version is unsupported");
        }
        let path = match native_path(arguments) {
            Ok(path) => path.to_owned(),
            Err(why) => return Output::error(why),
        };
        Some(match capability.operation {
            magi_proto::tooling::FileOperation::Read => magi_proto::permit::Action::Read { path },
            _ => magi_proto::permit::Action::Write { path },
        })
    } else {
        wants(&this.card, arguments)
    };
    if matches!(&action, Some(magi_proto::permit::Action::Network { host }) if host.is_empty()) {
        return Output::error(format!(
            "{} names no host to reach, so nobody can be asked about it",
            this.card.name
        ));
    }
    if let Some(action) = action
        && let Err(why) = ops.allow(&this.card.name, &action)
    {
        return Output::error(why);
    }
    let jail = ops.jail();
    let context = crate::holding::Context {
        environ: this.environ.clone(),
        configure: this.configured.clone(),
        jail: jail.clone(),
        cwd: Some(PathBuf::from(&call.cwd)),
        call: ops.call(),
    };
    if let Some(capability) = &this.card.file_revision {
        return native(this, call, context, capability, ops, cancel);
    }
    // A call may stop and ask, and then go on. Bounded, because a tool that asked forever would
    // hold the turn open forever. Wider than the two a person is ever asked, because a question
    // put to magi costs patience rather than attention: a search scores a batch at a time.
    for _ in 0..8 {
        let ran = match run_context(&this.program, &call, &context, cancel) {
            // A refusal is still something the model reads, and it can try another way round.
            Err(why) => return Output::error(why),
            Ok(ran) => ran,
        };
        // Rows a tool fills itself, the general form of a question: magi reserves the space and
        // drives the surface, and the answer comes back as an id and resumes the call.
        if let Some(Shown::Surface(surface)) = &ran.shown {
            let Some(chosen) = this
                .holds
                .hold(&this.card.name, surface, arguments, &context)
            else {
                return Output::error(format!(
                    "{} wanted the screen for {} and there was none",
                    this.card.name, surface.about
                ));
            };
            call.answered = Some(chosen);
            continue;
        }
        // A question for magi rather than for the person: answered out of the session and
        // handed straight back, so nothing is drawn and nobody is interrupted. A refusal
        // resumes the call too — a tool that asked for a model there is none of should say so
        // itself rather than have the call fail underneath it.
        if let Some(Shown::Wonder(wondering)) = &ran.shown {
            call.answered = Some(answered(&this.knows, wondering));
            continue;
        }
        let Some(Shown::Ask(ask)) = &ran.shown else {
            return finished(ran);
        };
        let Some(choice) = this.asks.ask(&this.card.name, ask) else {
            // Nobody answered. Told to the model, because a blank result reads as a tool that
            // silently does nothing.
            return Output::error(format!(
                "{} stopped to ask \"{}\" and nobody answered",
                this.card.name, ask.question
            ));
        };
        call.answered = Some(choice);
    }
    Output::error(format!(
        "{} kept asking rather than answering",
        this.card.name
    ))
}

fn native_path(arguments: &serde_json::Value) -> Result<&str, String> {
    for field in [
        "file_revision",
        "file_recovery",
        "revision_contract",
        "expected_revision",
        "mutation_id",
    ] {
        if arguments.get(field).is_some() {
            return Err("native file guards are coordinator-owned, outside model arguments".into());
        }
    }
    arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty() && path.len() <= 4096 && !path.as_bytes().contains(&0))
        .ok_or_else(|| "native file tools require a bounded nonempty path".into())
}

fn native(
    this: &SuppliedTool,
    mut call: Call,
    context: crate::holding::Context,
    capability: &magi_proto::tooling::FileCapability,
    ops: &dyn Ops,
    cancel: &dyn Cancel,
) -> Output {
    if cancel.is_cancelled() {
        return Output::error("native file call cancelled before admission");
    }
    if ops.cwd().to_str() != Some(call.cwd.as_str()) {
        return Output::error("native file cwd must be representable without loss");
    }
    let scope = match this.file_scope() {
        Some(scope) => scope,
        None => return Output::error("native file supplier identity is unavailable"),
    };
    let path = match native_path(&call.args) {
        Ok(path) => path,
        Err(why) => return Output::error(why),
    };
    let grant = match ops.file_revision(
        crate::files::Subject {
            supplier: &scope,
            cwd: &call.cwd,
            path,
        },
        capability,
    ) {
        Ok(grant) => grant,
        Err(why) => return Output::error(why),
    };
    call.revision = grant.revision().clone();
    if call.revision.capability().as_ref() != Some(capability) {
        return grant
            .failed("native file admission did not retain the selected tool capability".into());
    }
    let ran = match transport::run(&this.program, &call, &context, cancel) {
        Ok(ran) => ran,
        Err(why) if why.no_effect() => return grant.refused(why.into_string()),
        Err(why) => return grant.failed(why.into_string()),
    };
    let mut output = finished(ran);
    if matches!(
        output.shown,
        Some(Shown::Ask(_) | Shown::Surface(_) | Shown::Wonder(_))
    ) {
        output.is_error = true;
        output.shown = None;
        output.content.push_str(
            "\nNative file results cannot request interactive resumption; operation not retried.",
        );
    }
    grant.complete(output)
}
