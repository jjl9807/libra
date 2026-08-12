//! Completion implementation for the test-only fake provider.

use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
};

use tokio::time::sleep;

use super::fixture::{
    FakeFixture, FakeFixtureError, FakeMatchContext, FakeResponseAction, FakeStreamDelta,
};
use crate::internal::ai::completion::{
    AssistantContent, CompletionError, CompletionModel as CompletionModelTrait, CompletionRequest,
    CompletionResponse, CompletionStreamEvent, CompletionUsage, Function, Message, OneOrMany, Text,
    ToolCall, UserContent,
};

#[derive(Clone, Debug)]
pub struct Client {
    fixture: Arc<FakeFixture>,
    consumed: Arc<Mutex<HashSet<usize>>>,
}

impl Client {
    pub fn from_fixture_path(path: &Path) -> Result<Self, FakeFixtureError> {
        Ok(Self {
            fixture: Arc::new(FakeFixture::from_path(path)?),
            consumed: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    pub fn completion_model(&self, model: &str) -> CompletionModel {
        CompletionModel {
            fixture: self.fixture.clone(),
            consumed: self.consumed.clone(),
            model: model.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompletionModel {
    fixture: Arc<FakeFixture>,
    consumed: Arc<Mutex<HashSet<usize>>>,
    model: String,
}

impl CompletionModel {
    /// Model id this fake provider was constructed for. Mirrors the
    /// `model_name()` accessor on every production provider so the
    /// runtime adapter can fetch the id without branching on variant.
    pub fn model_name(&self) -> &str {
        &self.model
    }
}

#[derive(Clone, Debug)]
pub struct FakeRawResponse {
    pub model: String,
    pub matched_response_index: Option<usize>,
    pub usage: Option<crate::internal::ai::completion::CompletionUsageSummary>,
}

impl CompletionUsage for FakeRawResponse {
    fn usage_summary(&self) -> Option<crate::internal::ai::completion::CompletionUsageSummary> {
        self.usage.clone()
    }
}

impl CompletionModelTrait for CompletionModel {
    type Response = FakeRawResponse;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let latest_user_text = latest_user_text(&request).unwrap_or_default();
        let ctx = FakeMatchContext {
            latest_user_text: &latest_user_text,
            after_tool_result: last_user_is_tool_result(&request),
        };
        let (matched_response_index, action) = {
            let mut consumed = match self.consumed.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let (index, action) = self.fixture.select_ctx(&ctx, &consumed).ok_or_else(|| {
                CompletionError::ProviderError("no fake provider response matched".to_string())
            })?;
            if let Some(matched) = index
                && self
                    .fixture
                    .responses
                    .get(matched)
                    .is_some_and(|rule| rule.once)
            {
                consumed.insert(matched);
            }
            (index, action.clone())
        };

        if !action.delay().is_zero() {
            sleep(action.delay()).await;
        }

        if let Some(stream_events) = request.stream_events.as_ref() {
            for event in action.stream() {
                let event = match event {
                    FakeStreamDelta::Text { delta } => CompletionStreamEvent::TextDelta {
                        request_id: None,
                        delta: delta.clone(),
                    },
                    FakeStreamDelta::Thinking { delta } => CompletionStreamEvent::ThinkingDelta {
                        request_id: None,
                        delta: delta.clone(),
                    },
                };
                let _ = stream_events.send(event);
            }
        }

        let usage = action.usage();
        let content = match action {
            FakeResponseAction::Text { text, .. } => {
                vec![AssistantContent::Text(Text { text })]
            }
            FakeResponseAction::ToolCall {
                id,
                name,
                arguments,
                ..
            } => vec![AssistantContent::ToolCall(ToolCall {
                id,
                name: name.clone(),
                function: Function { name, arguments },
            })],
            FakeResponseAction::Error { message, .. } => {
                return Err(CompletionError::ProviderError(message));
            }
        };

        Ok(CompletionResponse {
            content,
            reasoning_content: None,
            raw_response: FakeRawResponse {
                model: self.model.clone(),
                matched_response_index,
                usage,
            },
        })
    }
}

fn last_user_is_tool_result(request: &CompletionRequest) -> bool {
    request
        .chat_history
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { content } => Some(
                content
                    .iter()
                    .any(|item| matches!(item, UserContent::ToolResult(_))),
            ),
            Message::Assistant { .. } | Message::System { .. } => None,
        })
        .unwrap_or(false)
}

fn latest_user_text(request: &CompletionRequest) -> Option<String> {
    request
        .chat_history
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User { content } | Message::System { content } => {
                Some(user_content_text(content))
            }
            Message::Assistant { .. } => None,
        })
}

fn user_content_text(content: &OneOrMany<UserContent>) -> String {
    content
        .iter()
        .filter_map(|item| match item {
            UserContent::Text(text) => Some(text.text.as_str()),
            UserContent::Image(_) | UserContent::ToolResult(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn fake_provider_returns_matching_text_response() {
        let fixture = FakeFixture {
            responses: vec![super::super::fixture::FakeResponseRule {
                matcher: super::super::fixture::FakeMatcher {
                    contains: Some("hello".to_string()),
                    equals: None,
                    after_tool_result: None,
                },
                once: false,
                action: FakeResponseAction::Text {
                    text: "fake hello".to_string(),
                    delay_ms: 0,
                    stream: vec![],
                    usage: None,
                },
            }],
            fallback: Some(FakeResponseAction::Error {
                message: "fallback".to_string(),
                delay_ms: 0,
            }),
        };
        let model = CompletionModel {
            fixture: Arc::new(fixture),
            consumed: Arc::new(Mutex::new(HashSet::new())),
            model: "fake".to_string(),
        };

        let response = model
            .completion(CompletionRequest::new(vec![Message::user("hello")]))
            .await
            .expect("fake response should succeed");

        assert!(matches!(
            response.content.as_slice(),
            [AssistantContent::Text(Text { text })] if text == "fake hello"
        ));
        assert_eq!(response.raw_response.matched_response_index, Some(0));
    }

    #[tokio::test]
    async fn fake_provider_can_return_tool_call() {
        let fixture = FakeFixture {
            responses: vec![super::super::fixture::FakeResponseRule {
                matcher: Default::default(),
                once: false,
                action: FakeResponseAction::ToolCall {
                    id: "call-1".to_string(),
                    name: "request_user_input".to_string(),
                    arguments: json!({"question":"Continue?"}),
                    delay_ms: 0,
                    stream: vec![],
                    usage: None,
                },
            }],
            fallback: None,
        };
        let model = CompletionModel {
            fixture: Arc::new(fixture),
            consumed: Arc::new(Mutex::new(HashSet::new())),
            model: "fake".to_string(),
        };

        let response = model
            .completion(CompletionRequest::new(vec![Message::user("ask")]))
            .await
            .expect("fake response should succeed");

        assert!(matches!(
            response.content.as_slice(),
            [AssistantContent::ToolCall(ToolCall { id, name, .. })]
                if id == "call-1" && name == "request_user_input"
        ));
    }
}
