//! Schema-shape contract tests. These do not require a running daemon —
//! schema introspection works without a socket, and resolver queries that
//! reach for the control socket surface a connection error envelope.

use std::sync::Arc;

use animus_plugin_protocol::{RpcNotification, RpcRequest, RpcResponse};
use animus_subject_protocol::{ChangeKind, Subject, SubjectChangedEvent, SubjectId, SubjectStatus};
use animus_transport_graphql::{
    config::GraphqlConfig,
    schema::{build_schema, build_schema_no_subs},
};
use chrono::Utc;
use futures_util::StreamExt;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{timeout, Duration};

fn cfg() -> Arc<GraphqlConfig> {
    Arc::new(GraphqlConfig::default())
}

fn cfg_with_socket(path: std::path::PathBuf) -> Arc<GraphqlConfig> {
    Arc::new(GraphqlConfig {
        control_socket_path: path,
        ..GraphqlConfig::default()
    })
}

#[tokio::test]
async fn introspection_includes_required_roots() {
    let schema = build_schema_no_subs(cfg());
    let sdl = schema.sdl();

    for needle in [
        // Roots
        "type QueryRoot",
        "type MutationRoot",
        // Rich domain types
        "type Subject",
        "type SubjectAttachment",
        "enum SubjectStatus",
        "type Workflow",
        "type WorkflowRunStart",
        "enum WorkflowStatus",
        "type QueueEntry",
        "type QueueStats",
        "enum QueueState",
        "type DaemonStatus",
        "type DaemonHealth",
        "type PluginHealth",
        "type DaemonAgent",
        "enum HealthStatus",
        "type Plugin",
        "type PluginRegistryEntry",
        // Key Subject fields (drift guard)
        "nativeStatus",
        "statusMetadata",
        "attachments",
        // Queries
        "workflows",
        "queue",
        "queueStats",
        "plugin",
        "pluginSearch",
        "daemon",
        "daemonHealth",
        "daemonAgents",
        "subject",
        "subjectNext",
        // Mutations
        "runWorkflow",
        "executeWorkflow",
        "enqueue",
        "installPlugin",
        "createSubject",
        "setSubjectStatus",
        "startDaemon",
    ] {
        assert!(
            sdl.contains(needle),
            "schema SDL missing expected fragment: {needle}\n--- SDL ---\n{sdl}"
        );
    }

    // Dropped surfaces must NOT appear (agent + project cruft).
    for forbidden in [
        "type Agent ",
        "enableAgent",
        "disableAgent",
        "Project",
        "agentRun",
    ] {
        assert!(
            !sdl.contains(forbidden),
            "schema SDL contains forbidden fragment: {forbidden}\n--- SDL ---\n{sdl}"
        );
    }
}

#[tokio::test]
async fn workflows_query_surfaces_connection_error_without_daemon() {
    let schema = build_schema_no_subs(cfg());
    let res = schema
        .execute("{ workflows { id definition status } }")
        .await;
    assert!(
        !res.errors.is_empty(),
        "expected control-socket error, got data: {:?}",
        res.data
    );
    let msg = res.errors[0].message.clone();
    assert!(
        msg.contains("control socket"),
        "expected control-socket failure, got: {msg}"
    );
}

#[tokio::test]
async fn queue_query_surfaces_connection_error_without_daemon() {
    let schema = build_schema_no_subs(cfg());
    let res = schema
        .execute("{ queue { id taskId priority state } }")
        .await;
    assert!(
        !res.errors.is_empty(),
        "expected control-socket error, got data: {:?}",
        res.data
    );
    let msg = res.errors[0].message.clone();
    assert!(
        msg.contains("control socket"),
        "expected control-socket failure, got: {msg}"
    );
}

#[tokio::test]
async fn queue_query_maps_entries_from_mock_daemon() {
    let tmp = TempDir::new().unwrap();
    let socket = tmp.path().join("control.sock");
    let listener = UnixListener::bind(&socket).expect("bind socket");

    tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.expect("accept");
        let (read_half, mut write_half) = conn.split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read request");
        let req: RpcRequest = serde_json::from_str(line.trim()).expect("decode request");
        assert_eq!(req.method, "queue/list");
        let resp = RpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(serde_json::json!({
                "entries": [
                    {
                        "id": "q-1",
                        "subject_id": "task:T1",
                        "status": "ready",
                        "priority": 3u8,
                        "enqueued_at": "2026-05-23T00:00:00Z"
                    },
                    {
                        "id": "q-2",
                        "subject_id": "task:T2",
                        "status": "held",
                        "priority": 1u8,
                        "enqueued_at": "2026-05-23T00:01:00Z",
                        "hold_reason": "manual"
                    },
                    {
                        "id": "q-3",
                        "subject_id": "task:T3",
                        "status": "in-flight",
                        "priority": 2u8,
                        "enqueued_at": "2026-05-23T00:02:00Z"
                    }
                ],
                "next_cursor": null
            })),
            error: None,
        };
        let mut frame = serde_json::to_string(&resp).unwrap();
        frame.push('\n');
        write_half.write_all(frame.as_bytes()).await.unwrap();
        write_half.flush().await.unwrap();
    });

    let schema = build_schema_no_subs(cfg_with_socket(socket));
    let res = schema
        .execute("{ queue { id taskId priority state held holdReason enqueuedAt } }")
        .await;
    assert!(res.errors.is_empty(), "unexpected errors: {:?}", res.errors);
    let data = res.data.into_json().unwrap();
    let entries = data["queue"].as_array().expect("queue array");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["id"], "q-1");
    assert_eq!(entries[0]["taskId"], "task:T1");
    // `state` is a GraphQL enum — serialized SCREAMING_SNAKE_CASE.
    assert_eq!(entries[0]["state"], "READY");
    assert_eq!(entries[0]["held"], false);
    assert_eq!(entries[0]["priority"], 3);
    assert_eq!(entries[1]["state"], "HELD");
    assert_eq!(entries[1]["held"], true);
    assert_eq!(entries[1]["holdReason"], "manual");
    assert_eq!(entries[2]["state"], "IN_FLIGHT");
}

fn sample_event(seq: u64) -> SubjectChangedEvent {
    let id = SubjectId::new(format!("native:T{seq}"));
    SubjectChangedEvent {
        id: id.clone(),
        change_kind: ChangeKind::Updated,
        subject: Subject {
            id,
            kind: "task".into(),
            title: format!("task {seq}"),
            description: None,
            status: SubjectStatus::Ready,
            priority: None,
            assignee: None,
            labels: vec![],
            parent: None,
            children: vec![],
            url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            custom: Default::default(),
            native_status: None,
            status_metadata: serde_json::Value::Null,
            attachments: vec![],
        },
        previous_native_status: None,
        previous_dispatch_label: None,
    }
}

#[tokio::test]
async fn subject_changed_subscription_streams_events_and_cancels_on_drop() {
    let tmp = TempDir::new().unwrap();
    let socket = tmp.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let received_cancel = Arc::new(AsyncMutex::new(false));
    let received_cancel_clone = Arc::clone(&received_cancel);

    tokio::spawn(async move {
        let (conn, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = conn.into_split();
        let write_half = Arc::new(AsyncMutex::new(write_half));
        let mut reader = BufReader::new(read_half);

        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let req: RpcRequest = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(req.method, "subject/watch");
        let req_id = req.id.clone();

        let ack = RpcResponse {
            jsonrpc: "2.0".into(),
            id: req_id.clone(),
            result: Some(serde_json::json!({ "watching": true })),
            error: None,
        };
        {
            let mut g = write_half.lock().await;
            let mut frame = serde_json::to_vec(&ack).unwrap();
            frame.push(b'\n');
            g.write_all(&frame).await.unwrap();
            g.flush().await.unwrap();
        }

        for i in 0..3 {
            let event = sample_event(i);
            let notification = RpcNotification::new(
                "subject/changed".to_string(),
                Some(serde_json::json!({
                    "id": req_id,
                    "data": event,
                })),
            );
            let mut g = write_half.lock().await;
            let mut frame = serde_json::to_vec(&notification).unwrap();
            frame.push(b'\n');
            g.write_all(&frame).await.unwrap();
            g.flush().await.unwrap();
            drop(g);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let mut cancel_line = String::new();
        if timeout(Duration::from_secs(2), reader.read_line(&mut cancel_line))
            .await
            .is_ok()
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(cancel_line.trim()) {
                if v.get("method").and_then(|m| m.as_str()) == Some("$/cancelRequest") {
                    *received_cancel_clone.lock().await = true;
                }
            }
        }
    });

    let schema = build_schema(cfg_with_socket(socket));
    let mut stream =
        schema.execute_stream("subscription { subjectChanged { subjectId change at } }");

    for expected in 0..3 {
        let resp = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("subscription recv timeout")
            .expect("stream closed early");
        assert!(resp.errors.is_empty(), "graphql errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let ev = &data["subjectChanged"];
        assert_eq!(ev["subjectId"], format!("native:T{expected}"));
        assert_eq!(ev["change"], "updated");
    }

    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        *received_cancel.lock().await,
        "server did not see $/cancelRequest"
    );
}

#[tokio::test]
async fn workflow_events_subscription_streams_events_and_cancels_on_drop() {
    let tmp = TempDir::new().unwrap();
    let socket = tmp.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let received_cancel = Arc::new(AsyncMutex::new(false));
    let received_cancel_clone = Arc::clone(&received_cancel);

    tokio::spawn(async move {
        let (conn, _) = listener.accept().await.unwrap();
        let (read_half, write_half) = conn.into_split();
        let write_half = Arc::new(AsyncMutex::new(write_half));
        let mut reader = BufReader::new(read_half);

        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let req: RpcRequest = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(req.method, "workflow/events");
        let req_id = req.id.clone();

        let ack = RpcResponse {
            jsonrpc: "2.0".into(),
            id: req_id.clone(),
            result: Some(serde_json::json!({ "watching": true })),
            error: None,
        };
        {
            let mut g = write_half.lock().await;
            let mut frame = serde_json::to_vec(&ack).unwrap();
            frame.push(b'\n');
            g.write_all(&frame).await.unwrap();
            g.flush().await.unwrap();
        }

        for i in 0..3u64 {
            let kind = if i % 2 == 0 {
                "phase_started"
            } else {
                "phase_completed"
            };
            let event = serde_json::json!({
                "workflow_id": "wf-1",
                "kind": kind,
                "payload": { "seq": i },
                "occurred_at": Utc::now().to_rfc3339(),
            });
            let notification = RpcNotification::new(
                "workflow/event".to_string(),
                Some(serde_json::json!({
                    "id": req_id,
                    "data": event,
                })),
            );
            let mut g = write_half.lock().await;
            let mut frame = serde_json::to_vec(&notification).unwrap();
            frame.push(b'\n');
            g.write_all(&frame).await.unwrap();
            g.flush().await.unwrap();
            drop(g);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let mut cancel_line = String::new();
        if timeout(Duration::from_secs(2), reader.read_line(&mut cancel_line))
            .await
            .is_ok()
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(cancel_line.trim()) {
                if v.get("method").and_then(|m| m.as_str()) == Some("$/cancelRequest") {
                    *received_cancel_clone.lock().await = true;
                }
            }
        }
    });

    let schema = build_schema(cfg_with_socket(socket));
    let mut stream =
        schema.execute_stream("subscription { workflowEvents { workflowId kind payload at } }");

    for expected in 0..3u64 {
        let resp = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("subscription recv timeout")
            .expect("stream closed early");
        assert!(resp.errors.is_empty(), "graphql errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let ev = &data["workflowEvents"];
        assert_eq!(ev["workflowId"], "wf-1");
        let expected_kind = if expected % 2 == 0 {
            "phase_started"
        } else {
            "phase_completed"
        };
        assert_eq!(ev["kind"], expected_kind);
        let payload: serde_json::Value =
            serde_json::from_str(ev["payload"].as_str().expect("payload string")).unwrap();
        assert_eq!(payload["seq"].as_u64(), Some(expected));
    }

    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        *received_cancel.lock().await,
        "server did not see $/cancelRequest"
    );
}
