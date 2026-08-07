//! 共享 HTTP 客户端配置。

use std::time::Duration;

pub const USER_AGENT: &str = "MaaTUI/0.1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(REQUEST_TIMEOUT)
        .timeout_write(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT)
        .build()
}
