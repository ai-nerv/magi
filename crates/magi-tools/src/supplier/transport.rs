use super::failure::Failure;
use super::*;

pub(super) fn run(
    program: &str,
    call: &Call,
    context: &crate::holding::Context,
    cancel: &dyn Cancel,
) -> Result<Ran, super::failure::Failure> {
    call.revision.validate().map_err(Failure::refused)?;
    if cancel.is_cancelled() {
        return Err(Failure::refused("tool call was cancelled before dispatch"));
    }
    let remote = call.tool.starts_with("mcp_");
    // The library door when asked for, and for MCP unless told otherwise. Only a call that never
    // left falls back to a spawn.
    if mcp::uses_library(remote, std::env::var(DOOR).ok().as_deref())
        && let Some((path, key, file_admission, request_context)) = door::serving(program, context)
    {
        if cancel.is_cancelled() {
            return Err(Failure::refused("tool call was cancelled before dispatch"));
        }
        if call.revision.requested() && !file_admission {
            return Err(Failure::refused(
                "the selected tools daemon does not enforce native file revisions; guarded call not sent",
            ));
        }
        match door::over_socket_context(&path, &key, call, cancel, request_context) {
            Ok(ran) => return Ok(ran),
            Err(door::Failed::Sent(why)) if remote => {
                return Err(Failure::unknown(format!(
                    "MCP response lost: {why}; the remote operation may have taken effect; not retried"
                )));
            }
            Err(door::Failed::Sent(why)) => return Err(why.prefixed(&format!("{program}: "))),
            Err(door::Failed::Unsent(why)) => {
                magi_model::noted!("tools: {program} socket call fell back to a spawn: {why}");
            }
        }
    }
    let body = serde_json::to_vec(call)
        .map_err(|why| Failure::refused(format!("this call will not encode: {why}")))?;
    if body.len() > crate::family::FRAME_CAP {
        return Err(Failure::refused(
            "tool call exceeds the family frame bound; request not sent",
        ));
    }
    let mut spawning = std::process::Command::new(program);
    spawning.arg("run");
    context.apply(&mut spawning);
    if call.revision.requested() {
        spawning.arg("--file-admission");
    }
    let out = waiting::run_known(
        spawning,
        body,
        cancel,
        door::PATIENCE,
        call.revision.requested(),
    )
    .map_err(|why| why.prefixed(&format!("{program} ")))?;
    let first = rows_known(&out)
        .map_err(|why| why.prefixed(&format!("{program} ")))?
        .into_iter()
        .next()
        .ok_or_else(|| Failure::unknown(format!("{program} answered nothing")))?;
    admission::result(first, call).map_err(|why| Failure::unknown(format!("{program}: {why}")))
}
