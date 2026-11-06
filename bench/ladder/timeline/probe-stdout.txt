# Timeline probe: navigera serve vs go-rod, 9 interleaved runs each (run 42227749333)

All values ms (median, IQR bounds). `net goto` = the goto window without browser launch.

| phase | navigera | go-rod | nv - rod |
|---|---|---|---|
| launch (nv launch_total / rod browser launch) | 211.0 (203.8-226.5) | 118.8 (112.2-146.5) | 92.2 |
| goto client total (incl. launch) | 335.5 (315.0-359.8) | 106.1 (96.4-118.8) | 229.4 |
| net goto (nv: total - launch; rod: goto_ms) | 121.0 (107.5-141.4) | 106.1 (96.4-118.8) | 14.9 |
|   Navigate command reply (rod client) | - (---) | 46.5 (45.2-50.6) | - |
|   nav_start -> responseReceived (client clock) | - (---) | 46.9 (44.7-50.8) | - |
|   responseReceived -> loadEventFired (client) | - (---) | 63.2 (52.4-75.2) | - |
|   CDP loadEventFired receipt - WaitLoad return | - (---) | 4.4 (0.2-6.8) | - |
| first eval (client) | 0.4 (0.3-0.5) | 5.6 (2.7-8.4) | -5.2 |
| first eval (engine/CDP only) | 0.3 (0.2-0.3) | - (---) | - |

go-rod network detail (from Network.responseReceived timing):

| segment | median (IQR) |
|---|---|
| DNS | 0.0 (0.0-0.0) |
| connect (TCP) | 13.4 (8.9-15.1) |
| TLS | 13.4 (8.9-15.1) |
| send | 0.2 (0.1-0.5) |
| request start -> first byte (receiveHeadersEnd) | 38.6 (34.1-42.0) |

go-rod CDP-clock splits (event monotonic timestamps):

| segment | median (IQR) |
|---|---|
| requestWillBeSent -> responseReceived | 41.8 (39.1-45.1) |
| responseReceived -> loadEventFired | 55.7 (46.2-71.2) |

