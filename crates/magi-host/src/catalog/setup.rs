use magi_proto::ask::{Card, SetupState};

#[must_use]
pub fn requirement(card: &Card) -> String {
    if card.ready {
        return String::new();
    }
    let Some(setup) = &card.setup else {
        return card.needs.as_ref().map_or_else(
            || format!("{} is not reachable", card.provider),
            |variable| format!("set {variable}"),
        );
    };
    match setup.state {
        SetupState::CredentialMissing if !setup.variables.is_empty() => {
            format!(
                "set {}",
                setup
                    .variables
                    .iter()
                    .map(|group| group.join(" or "))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )
        }
        SetupState::CredentialMissing if setup.login => format!("sign in to {}", card.provider),
        SetupState::CredentialMissing => format!("configure credentials for {}", card.provider),
        SetupState::CredentialExpired => {
            format!("sign in to {} again; credentials expired", card.provider)
        }
        SetupState::CredentialUnreadable => "credential store cannot be read".into(),
        SetupState::ConfigurationIncomplete => format!("configure {}", setup.fields.join(", ")),
        SetupState::ProtocolUnavailable => format!("protocol {} is not available", card.api),
        SetupState::AuthUnavailable => "authentication method is not implemented".into(),
        SetupState::Ready | SetupState::Unknown => "provider readiness is not confirmed".into(),
    }
}

#[must_use]
pub fn wanted_variables(card: &Card) -> Vec<String> {
    if card.ready {
        return Vec::new();
    }
    match &card.setup {
        Some(setup) if setup.state == SetupState::CredentialMissing => setup
            .variables
            .iter()
            .filter_map(|group| group.first().cloned())
            .collect(),
        Some(_) => Vec::new(),
        None => card.needs.clone().into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn legacy_cards_and_structured_requirements_remain_distinct() {
        let mut value =
            json!({"id":"p/m","provider":"p","name":"m","api":"openai-completions","needs":"KEY"});
        let legacy: Card = serde_json::from_value(value.clone()).expect("legacy card");
        assert_eq!(requirement(&legacy), "set KEY");
        assert_eq!(wanted_variables(&legacy), ["KEY"]);
        value["setup"] = json!({"state":"credential_missing","methods":["api-key"],"variables":[["KEY","ALIAS"],["PROJECT"]]});
        let card: Card = serde_json::from_value(value.clone()).expect("setup");
        assert_eq!(requirement(&card), "set KEY or ALIAS and PROJECT");
        assert_eq!(wanted_variables(&card), ["KEY", "PROJECT"]);
        value["setup"]["state"] = json!("protocol_unavailable");
        let card: Card = serde_json::from_value(value.clone()).expect("unavailable");
        assert!(requirement(&card).contains("protocol"));
        assert!(wanted_variables(&card).is_empty());
        value["setup"]["state"] = json!("a_future_state");
        let card: Card = serde_json::from_value(value).expect("future state");
        assert_eq!(
            card.setup.as_ref().expect("setup").state,
            SetupState::Unknown
        );
        assert!(!requirement(&card).is_empty());
        assert!(wanted_variables(&card).is_empty());
        let (writer, reader) = tokio::io::duplex(8192);
        magi_ipc::FrameWriter::new(writer)
            .write(&card)
            .await
            .expect("CBOR frame");
        let received: Card = magi_ipc::FrameReader::new(reader)
            .read()
            .await
            .expect("round trip");
        assert_eq!(received, card);
    }
}
