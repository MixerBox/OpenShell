// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Namespace-aware TCP connection helper.
//!
//! Provides [`connect_in_netns`] which creates a TCP connection inside a
//! sandbox network namespace. Used by both the SSH server (for `direct-tcpip`
//! channel forwarding) and TCP port forwarding.

use std::os::fd::RawFd;

/// Connect a TCP stream to `addr` inside the sandbox network namespace.
///
/// The supervisor runs in the host network namespace while sandbox child
/// processes run in an isolated network namespace (with their own loopback).
/// A plain `TcpStream::connect("127.0.0.1:port")` from the supervisor would
/// hit the host loopback, not the sandbox loopback where services are listening.
///
/// On Linux, we spawn a dedicated OS thread, call `setns` to enter the sandbox
/// namespace, create the socket there, then convert it to a tokio `TcpStream`.
/// We use `std::thread::spawn` (not `spawn_blocking`) because `setns` changes
/// the calling thread's network namespace permanently — a tokio blocking-pool
/// thread could be reused for unrelated tasks and must not be contaminated.
/// On non-Linux platforms (no network namespace support), we connect directly.
pub async fn connect_in_netns(
    addr: &str,
    netns_fd: Option<RawFd>,
) -> std::io::Result<tokio::net::TcpStream> {
    #[cfg(target_os = "linux")]
    if let Some(fd) = netns_fd {
        let addr = addr.to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| -> std::io::Result<std::net::TcpStream> {
                // Enter the sandbox network namespace on this dedicated thread.
                // SAFETY: setns is safe to call; this is a dedicated thread that
                // will exit after the connection is established.
                #[allow(unsafe_code)]
                let rc = unsafe { libc::setns(fd, libc::CLONE_NEWNET) };
                if rc != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                std::net::TcpStream::connect(&addr)
            })();
            let _ = tx.send(result);
        });

        let std_stream = rx
            .await
            .map_err(|_| std::io::Error::other("netns connect thread panicked"))??;
        std_stream.set_nonblocking(true)?;
        return tokio::net::TcpStream::from_std(std_stream);
    }

    #[cfg(not(target_os = "linux"))]
    let _ = netns_fd;

    tokio::net::TcpStream::connect(addr).await
}
