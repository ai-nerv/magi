use super::{Trouble, cards_using};
use magi_model::{Content, Context, Role, ToolStrictness};
use magi_proto::ask::Refusal;
use std::collections::BTreeMap;

pub(super) async fn check(
    program: &str,
    environ: &BTreeMap<String, String>,
    model: &str,
    context: &Context,
) -> Result<(), Trouble> {
    let required = required(context).map_err(|message| Trouble {
        message: message.into(),
        why: Refusal::Invalid,
        overflow: None,
    })?;
    if !required {
        return Ok(());
    }
    let refused = || Trouble {
        message: "The selected model role does not confirm support for required strict tools."
            .into(),
        why: Refusal::Invalid,
        overflow: None,
    };
    let cards = cards_using(program, environ).await.map_err(|_| refused())?;
    if cards.iter().any(|card| {
        card.id == model
            && card
                .capabilities
                .as_ref()
                .and_then(|caps| caps.strict_tools)
                == Some(true)
    }) {
        Ok(())
    } else {
        Err(refused())
    }
}

fn required(context: &Context) -> Result<bool, &'static str> {
    if !context
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .any(|c| matches!(c, Content::ToolChanges { .. }))
    {
        return Ok(context
            .tools
            .iter()
            .any(|tool| tool.strict == Some(ToolStrictness::Require)));
    }
    let mut count = context.tools.len();
    if count > 16384 {
        return Err("tool history exceeds 16384 declarations and removals");
    }
    let mut current = BTreeMap::new();
    fn add<'a>(
        current: &mut BTreeMap<&'a str, bool>,
        tool: &'a magi_model::Tool,
    ) -> Result<(), &'static str> {
        current.insert(
            tool.name.as_str(),
            tool.strict == Some(ToolStrictness::Require),
        );
        if current.len() > 4096 {
            Err("tool history exceeds 4096 active tools")
        } else {
            Ok(())
        }
    }
    for tool in &context.tools {
        add(&mut current, tool)?;
    }
    for message in &context.messages {
        let mut blocks = 0;
        for content in &message.content {
            if let Content::ToolChanges { added, removed } = content {
                blocks += 1;
                if message.role != Role::System || blocks > 1 {
                    return Err("tool changes require one block per System message");
                }
                count = count
                    .saturating_add(added.len())
                    .saturating_add(removed.len());
                if count > 16384 {
                    return Err("tool history exceeds 16384 declarations and removals");
                }
                for name in removed {
                    current.remove(name.as_str());
                }
                for tool in added {
                    add(&mut current, tool)?;
                }
            }
        }
    }
    Ok(current.values().any(|required| *required))
}

#[cfg(test)]
mod tests;
