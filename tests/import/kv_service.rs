// Copyright 2018 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;

use futures::{stream, Future, Stream};
use tempdir::TempDir;
use uuid::Uuid;

use grpc::{ChannelBuilder, Environment, Result, WriteFlags};
use kvproto::import_kvpb::*;
use kvproto::import_kvpb_grpc::*;

use tikv::config::TiKvConfig;
use tikv::import::ImportKVServer;

fn new_kv_server() -> (ImportKVServer, ImportKvClient) {
    let temp_dir = TempDir::new("test_import_kv_server").unwrap();

    let mut cfg = TiKvConfig::default();
    cfg.server.addr = "127.0.0.1:0".to_owned();
    cfg.import.import_dir = temp_dir.path().to_str().unwrap().to_owned();
    let server = ImportKVServer::new(&cfg);

    let ch = {
        let env = Arc::new(Environment::new(1));
        let addr = server.bind_addrs().first().unwrap();
        ChannelBuilder::new(env).connect(&format!("{}:{}", addr.0, addr.1))
    };
    let client = ImportKvClient::new(ch);

    (server, client)
}

#[test]
fn test_kv_service() {
    let (mut server, client) = new_kv_server();
    server.start();

    let uuid = Uuid::new_v4().as_bytes().to_vec();
    let mut head = WriteHead::new();
    head.set_uuid(uuid.clone());

    let mut m = Mutation::new();
    m.op = Mutation_OP::Put;
    m.set_key(vec![1]);
    m.set_value(vec![1]);
    let mut batch = WriteBatch::new();
    batch.set_commit_ts(123);
    batch.mut_mutations().push(m);

    let mut open = OpenEngineRequest::new();
    open.set_uuid(uuid.clone());

    let mut close = CloseEngineRequest::new();
    close.set_uuid(uuid.clone());

    // Write an engine before it is opened.
    // Only send the write head here to avoid other gRPC errors.
    let resp = send_write_head(&client, &head).unwrap();
    assert!(resp.get_error().has_engine_not_found());

    // Close an engine before it it opened.
    let resp = client.close_engine(&close).unwrap();
    assert!(resp.get_error().has_engine_not_found());

    client.open_engine(&open).unwrap();
    let resp = send_write(&client, &head, &batch).unwrap();
    assert!(!resp.has_error());
    let resp = send_write(&client, &head, &batch).unwrap();
    assert!(!resp.has_error());

    let mut m = Mutation::new();
    m.op = Mutation_OP::Put;
    m.set_key(vec![2]);
    m.set_value(vec![0; 90_000_000]);
    let mut huge_batch = WriteBatch::new();
    huge_batch.set_commit_ts(124);
    huge_batch.mut_mutations().push(m);
    let resp = send_write(&client, &head, &huge_batch).unwrap();
    assert!(!resp.has_error());

    let resp = client.close_engine(&close).unwrap();
    assert!(!resp.has_error());

    server.shutdown();
}

fn send_write(
    client: &ImportKvClient,
    head: &WriteHead,
    batch: &WriteBatch,
) -> Result<WriteEngineResponse> {
    let mut r1 = WriteEngineRequest::new();
    r1.set_head(head.clone());
    let mut r2 = WriteEngineRequest::new();
    r2.set_batch(batch.clone());
    let mut r3 = WriteEngineRequest::new();
    r3.set_batch(batch.clone());
    let reqs: Vec<_> = vec![r1, r2, r3]
        .into_iter()
        .map(|r| (r, WriteFlags::default()))
        .collect();
    let (tx, rx) = client.write_engine().unwrap();
    let stream = stream::iter_ok(reqs);
    stream.forward(tx).and_then(|_| rx).wait()
}

fn send_write_head(client: &ImportKvClient, head: &WriteHead) -> Result<WriteEngineResponse> {
    let mut req = WriteEngineRequest::new();
    req.set_head(head.clone());
    let (tx, rx) = client.write_engine().unwrap();
    let stream = stream::once(Ok((req, WriteFlags::default())));
    stream.forward(tx).and_then(|_| rx).wait()
}