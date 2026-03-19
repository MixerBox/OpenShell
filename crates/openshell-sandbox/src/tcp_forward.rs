// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! TCP port forwarding from the outer (host) namespace to the inner (sandbox)
//! network namespace.
//!
//! For each configured port, a `TcpListener` binds on `0.0.0.0:<port>` in the
//! outer namespace. Incoming connections are bridged to `127.0.0.1:<port>` in
//! the sandbox namespace using the same `connect_in_netns` pattern proven by
//! the SSH `direct-tcpip` handler.
//!
//! This enables K8s services, readiness probes, and ingress to reach services
//! running inside the sandbox without requiring SSH tunnels.

use std::os::fd::RawFd;

use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::netns_connect::connect_in_netns;

/// Spawn TCP port forwarders for the given ports.
///
/// Each port gets a listener on `0.0.0.0:<port>` in the outer namespace,
/// bridging connections to `127.0.0.1:<port>` in the inner namespace.
pub async fn spawn_tcp_forwards(ports: Vec<u16>, netns_fd: Option<RawFd>) {
    for port in ports {
        let fd = netns_fd;
        tokio::spawn(async move {
            if let Err(e) = run_forward(port, fd).await {
                warn!(port, error = %e, "TCP forward listener failed");
            }
        });
    }
}

async fn run_forward(port: u16, netns_fd: Option<RawFd>) -> std::io::Result<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!(port, "TCP port forward listening");

    loop {
        let (inbound, peer) = listener.accept().await?;
        let fd = netns_fd;
        tokio::spawn(async move {
            let target = format!("127.0.0.1:{port}");
            match connect_in_netns(&target, fd).await {
                Ok(outbound) => {
                    let (mut ri, mut wi) = tokio::io::split(inbound);
                    let (mut ro, mut wo) = tokio::io::split(outbound);
                    let _ = tokio::try_join!(
                        tokio::io::copy(&mut ri, &mut wo),
                        tokio::io::copy(&mut ro, &mut wi),
                    );
                }
                Err(e) => {
                    warn!(port, %peer, error = %e, "Failed to connect to inner namespace");
                }
            }
        });
    }
}
