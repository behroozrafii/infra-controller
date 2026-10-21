/*
 * SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 * SPDX-License-Identifier: Apache-2.0
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Request-level proofs for DPF trace annotations.

use std::future::Future;
use std::sync::Once;
use std::time::Duration;

use http::{Method, Request, Response, StatusCode};
use kube::Client;
use kube::client::Body;
use opentelemetry::trace::{TraceContextExt, TracerProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use serde_json::{Value, json};
use tower_test::mock;
use tracing::Instrument;
use tracing::instrument::WithSubscriber;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::prelude::*;

use super::KubeRepository;
use super::tests::detached_interface;
use crate::error::DpfError;
use crate::repository::{
    DpfOperatorConfigRepository, DpuDeploymentRepository, DpuDeviceRepository,
    DpuNodeMaintenanceRepository, DpuNodeRepository, DpuServiceInterfaceRepository,
};

const TRACEPARENT: &str = "tracing.k8s.io/traceparent";
const TRACESTATE: &str = "tracing.k8s.io/tracestate";
const TRACE_ID: &str = "00000000000000000000000000000042";

async fn with_trace<T>(state: Option<&str>, action: impl Future<Output = T>) -> T {
    static PROPAGATOR: Once = Once::new();
    PROPAGATOR.call_once(|| {
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    });
    let Some(state) = state else {
        // The propagator is installed for the whole process; absence means no active span.
        return action
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await;
    };
    let provider = SdkTracerProvider::builder().build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("dpf-trace-test")));
    let dispatch = tracing::Dispatch::new(subscriber);
    let span = tracing::dispatcher::with_default(&dispatch, || tracing::info_span!("dpf_write"));
    let headers = http::HeaderMap::from_iter([
        (
            http::HeaderName::from_static("traceparent"),
            format!("00-{TRACE_ID}-0000000000000055-01")
                .parse()
                .unwrap(),
        ),
        (
            http::HeaderName::from_static("tracestate"),
            state.parse().unwrap(),
        ),
    ]);
    trace_propagation::set_span_parent_from_headers(&span, &headers);
    action.instrument(span).with_subscriber(dispatch).await
}

fn current_traceparent() -> String {
    let context = tracing::Span::current().context();
    let span = context.span();
    let context = span.span_context();
    assert_eq!(context.trace_id().to_string(), TRACE_ID);
    assert_ne!(context.span_id().to_string(), "0000000000000055");
    format!("00-{TRACE_ID}-{}-01", context.span_id())
}

#[tokio::test]
async fn create_and_apply_propagate_trace_without_mutating_input() {
    for (scenario, create, state, annotations) in [
        (
            "create preserves metadata",
            true,
            Some("vendor=value"),
            true,
        ),
        (
            "apply initializes annotations",
            false,
            Some("vendor=value"),
            false,
        ),
        ("apply removes stale state", false, Some(""), true),
        ("create omits empty state", true, Some(""), false),
        ("no active span preserves payload", true, None, true),
        ("no active span adds no metadata", false, None, false),
    ] {
        with_trace(state, async {
            let (service, mut handle) = mock::pair::<Request<Body>, Response<Body>>();
            let repository = KubeRepository::with_client(Client::new(service, "default"));
            let mut resource = detached_interface();
            resource.metadata.labels = Some([("owner".into(), "nico".into())].into());
            if annotations {
                resource.metadata.annotations = Some(
                    [
                        ("example.com/keep".into(), "keep".into()),
                        (TRACEPARENT.into(), "old-parent".into()),
                        (TRACESTATE.into(), "old=state".into()),
                    ]
                    .into(),
                );
            }
            let original = serde_json::to_value(&resource).unwrap();
            let mut expected = original.clone();
            if let Some(state) = state {
                expected["metadata"]["annotations"][TRACEPARENT] = current_traceparent().into();
                if state.is_empty() {
                    expected["metadata"]["annotations"]
                        .as_object_mut()
                        .unwrap()
                        .remove(TRACESTATE);
                } else {
                    expected["metadata"]["annotations"][TRACESTATE] = state.into();
                }
            }
            let write = async {
                if create {
                    DpuServiceInterfaceRepository::create(&repository, &resource).await
                } else {
                    DpuServiceInterfaceRepository::apply(&repository, &resource).await
                }
            };
            let inspect = async {
                let (request, response) = handle.next_request().await.unwrap();
                assert_eq!(
                    request.method(),
                    if create { Method::POST } else { Method::PATCH }
                );
                assert_eq!(
                    request.headers()[http::header::CONTENT_TYPE],
                    if create {
                        "application/json"
                    } else {
                        "application/apply-patch+yaml"
                    }
                );
                assert!(
                    request
                        .uri()
                        .path()
                        .contains("/namespaces/test-ns/dpuserviceinterfaces")
                );
                let body = request.into_body().collect_bytes().await.unwrap();
                let actual: Value = serde_yaml::from_slice(&body).unwrap();
                assert_eq!(actual, expected, "{scenario}");
                response.send_response(
                    Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(expected.to_string().into_bytes()))
                        .unwrap(),
                );
            };
            let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(write, inspect)
            })
            .await
            .expect(scenario);
            result.unwrap();
            assert_eq!(
                serde_json::to_value(resource).unwrap(),
                original,
                "{scenario}"
            );
        })
        .await;
    }
}

#[derive(Clone, Copy)]
enum MergeTarget {
    Device,
    Node,
    Maintenance,
    Deployment,
    OperatorConfig,
}

#[tokio::test]
async fn merge_patches_propagate_trace_and_preserve_patch_semantics() {
    for (scenario, target, state, patch, inject) in [
        (
            "device metadata preserved",
            MergeTarget::Device,
            Some("vendor=value"),
            json!({
                "metadata": {"resourceVersion": "17", "labels": {"keep": "label"},
                    "annotations": {"example.com/keep": "yes", "example.com/remove": null}},
                "spec": {"serialNumber": "test-serial"},
            }),
            true,
        ),
        (
            "node initializes metadata",
            MergeTarget::Node,
            Some("vendor=value"),
            json!({"spec": {"nodeRebootMethod": null}}),
            true,
        ),
        (
            "maintenance initializes annotations",
            MergeTarget::Maintenance,
            Some("vendor=value"),
            json!({"metadata": {"labels": {"keep": "yes"}}}),
            true,
        ),
        (
            "deployment clears stale state",
            MergeTarget::Deployment,
            Some(""),
            json!({"metadata": {"annotations": {
                "example.com/keep": "yes", (TRACEPARENT): "old-parent", (TRACESTATE): "old=state",
            }}}),
            true,
        ),
        (
            "operator config propagates",
            MergeTarget::OperatorConfig,
            Some("vendor=value"),
            json!({"spec": {}}),
            true,
        ),
        (
            "no active span",
            MergeTarget::Device,
            None,
            json!({"spec": {"serialNumber": "test-serial"}}),
            false,
        ),
        (
            "explicit annotation deletion",
            MergeTarget::Device,
            Some("vendor=value"),
            json!({"metadata": {"annotations": null}}),
            false,
        ),
        (
            "explicit metadata deletion",
            MergeTarget::Device,
            Some("vendor=value"),
            json!({"metadata": null}),
            false,
        ),
        (
            "invalid annotation shape",
            MergeTarget::Device,
            Some("vendor=value"),
            json!({"metadata": {"annotations": []}}),
            false,
        ),
        (
            "invalid root shape",
            MergeTarget::Device,
            Some("vendor=value"),
            json!([]),
            false,
        ),
    ] {
        with_trace(state, async {
            let (service, mut handle) = mock::pair::<Request<Body>, Response<Body>>();
            let repository = KubeRepository::with_client(Client::new(service, "default"));
            let mut expected = patch.clone();
            if inject {
                expected["metadata"]["annotations"][TRACEPARENT] = current_traceparent().into();
                expected["metadata"]["annotations"][TRACESTATE] = match state {
                    Some("") => Value::Null,
                    Some(state) => state.into(),
                    None => unreachable!(),
                };
            }
            let write = async {
                match target {
                    MergeTarget::Device => DpuDeviceRepository::patch(&repository, "test", "test-ns", patch).await,
                    MergeTarget::Node => DpuNodeRepository::patch(&repository, "test", "test-ns", patch).await,
                    MergeTarget::Maintenance => DpuNodeMaintenanceRepository::patch(&repository, "test", "test-ns", patch).await,
                    MergeTarget::Deployment => DpuDeploymentRepository::patch(&repository, "test", "test-ns", patch).await,
                    MergeTarget::OperatorConfig => DpfOperatorConfigRepository::patch(&repository, "test", "test-ns", patch).await,
                }
            };
            let inspect = async {
                let (request, response) = handle.next_request().await.unwrap();
                assert_eq!(request.method(), Method::PATCH);
                assert_eq!(request.headers()[http::header::CONTENT_TYPE], "application/merge-patch+json");
                let body = request.into_body().collect_bytes().await.unwrap();
                let actual: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(actual, expected, "{scenario}");
                // The transport error must survive annotation injection unchanged.
                response.send_response(Response::builder().status(StatusCode::CONFLICT)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"status": "Failure", "message": "stale version",
                        "reason": "Conflict", "code": 409}).to_string().into_bytes())).unwrap());
            };
            let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(write, inspect)
            }).await.expect(scenario);
            assert!(matches!(result, Err(DpfError::KubeError(kube::Error::Api(error))) if error.code == 409), "{scenario}");
        }).await;
    }
}
