// Copyright (c) 2017 Chef Software Inc. and/or applicable contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use builder_core::metrics::CounterMetric;
use hab_http::ApiClient;
use hyper::client::IntoUrl;
use hyper::header::{qitem, Accept, Authorization, Bearer, UserAgent};
use hyper::mime::{Mime, SubLevel, TopLevel};
use hyper::status::StatusCode;
use hyper::{self, Url};
use jwt;
use serde_json;

use config::GitHubCfg;
use error::{HubError, HubResult};
use metrics::Counter;
use types::*;

const USER_AGENT: &'static str = "Habitat-Builder";

pub type TokenString = String;
pub type InstallationId = u32;

/// Bundle up a Github token with the Github App installation ID used
/// to create it.
///
/// Consumers will treat this as an opaque type; its main utility is
/// in carrying the installation ID around so we can generate metrics
/// on a per-installation basis.
pub struct AppToken {
    inner_token: TokenString,

    // Leave this here in anticipation of using it for tagged metrics
    #[allow(dead_code)]
    installation_id: InstallationId,
}

impl AppToken {
    // Not public, because you should only get these from
    // `GitHubClient::app_installation_token`
    fn new(inner_token: TokenString, installation_id: InstallationId) -> Self {
        AppToken {
            inner_token,
            installation_id,
        }
    }

    // Only providing this for builder-worker's benefit... it
    // currently needs a token for cloning via libgit2; once that's
    // gone, just delete this function.
    /// Retrieve the actual token content for use in HTTP calls.
    pub fn inner_token(&self) -> &str {
        self.inner_token.as_ref()
    }
}

#[derive(Clone)]
pub struct GitHubClient {
    pub api_url: String,
    app_id: u32,
    app_private_key: String,
    pub webhook_secret: String,
}

impl GitHubClient {
    pub fn new(config: GitHubCfg) -> Self {
        GitHubClient {
            api_url: config.api_url,
            app_id: config.app_id,
            app_private_key: config.app_private_key,
            webhook_secret: config.webhook_secret,
        }
    }

    pub fn app(&self) -> HubResult<App> {
        let app_token = generate_app_token(&self.app_private_key, &self.app_id);
        let url = Url::parse(&format!("{}/app", self.api_url)).map_err(HubError::HttpClientParse)?;
        let mut rep = http_get(url, Some(app_token))?;
        let mut body = String::new();
        rep.read_to_string(&mut body)?;
        debug!("GitHub response body, {}", body);
        if rep.status != StatusCode::Ok {
            let err: HashMap<String, String> = serde_json::from_str(&body)?;
            return Err(HubError::ApiError(rep.status, err));
        }
        let contents = serde_json::from_str::<App>(&body)?;
        Ok(contents)
    }

    pub fn app_installation_token(&self, install_id: u32) -> HubResult<AppToken> {
        let app_token = generate_app_token(&self.app_private_key, &self.app_id);
        let url = Url::parse(&format!(
            "{}/installations/{}/access_tokens",
            self.api_url, install_id
        )).map_err(HubError::HttpClientParse)?;

        Counter::InstallationToken.increment();
        let mut rep = http_post(url, Some(app_token))?;
        let mut body = String::new();
        rep.read_to_string(&mut body)?;
        debug!("GitHub response body, {}", body);
        match serde_json::from_str::<AppInstallationToken>(&body) {
            Ok(msg) => Ok(AppToken::new(msg.token, install_id)),
            Err(_) => {
                let err = serde_json::from_str::<AppAuthErr>(&body)?;
                Err(HubError::AppAuth(err))
            }
        }
    }

    /// Returns the contents of a file or directory in a repository.
    pub fn contents(&self, token: &AppToken, repo: u32, path: &str) -> HubResult<Option<Contents>> {
        let url = Url::parse(&format!(
            "{}/repositories/{}/contents/{}",
            self.api_url, repo, path
        )).map_err(HubError::HttpClientParse)?;

        Counter::Api("contents").increment();
        let mut rep = http_get(url, Some(&token.inner_token))?;
        let mut body = String::new();
        rep.read_to_string(&mut body)?;
        debug!("GitHub response body, {}", body);
        match rep.status {
            StatusCode::NotFound => return Ok(None),
            StatusCode::Ok => (),
            status => {
                let err: HashMap<String, String> = serde_json::from_str(&body)?;
                return Err(HubError::ApiError(status, err));
            }
        }
        let mut contents: Contents = serde_json::from_str(&body)?;
        // We need to strip line feeds as the Github API has started to return
        // base64 content with line feeds.
        if contents.encoding == "base64" {
            contents.content = contents.content.replace("\n", "");
        }
        Ok(Some(contents))
    }

    pub fn repo(&self, token: &AppToken, repo: u32) -> HubResult<Option<Repository>> {
        let url = Url::parse(&format!("{}/repositories/{}", self.api_url, repo)).unwrap();
        Counter::Api("repo").increment();
        let mut rep = http_get(url, Some(&token.inner_token))?;
        let mut body = String::new();
        rep.read_to_string(&mut body)?;
        debug!("GitHub response body, {}", body);
        match rep.status {
            StatusCode::NotFound => return Ok(None),
            StatusCode::Ok => (),
            status => {
                let err: HashMap<String, String> = serde_json::from_str(&body)?;
                return Err(HubError::ApiError(status, err));
            }
        }
        let value = serde_json::from_str(&body)?;
        Ok(Some(value))
    }

    // The main purpose of this is just to verify HTTP communication with GH.
    // There's nothing special about this endpoint, only that it doesn't require
    // auth and the response body seemed small. We don't even care what the
    // response is. For our purposes, just receiving a response is enough.
    pub fn meta(&self) -> HubResult<()> {
        let url = Url::parse(&format!("{}/meta", self.api_url)).unwrap();
        let mut rep = http_get(url, None::<String>)?;
        let mut body = String::new();
        rep.read_to_string(&mut body)?;
        debug!("GitHub response body, {}", body);

        if rep.status != StatusCode::Ok {
            let err: HashMap<String, String> = serde_json::from_str(&body)?;
            return Err(HubError::ApiError(rep.status, err));
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct RepositoryList {
    pub total_count: u32,
    pub repositories: Vec<Repository>,
}

fn generate_app_token<T, U>(key_path: T, app_id: U) -> String
where
    T: ToString,
    U: ToString,
{
    let mut payload = jwt::Payload::new();
    let header = jwt::Header::new(jwt::Algorithm::RS256);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let expiration = now + Duration::from_secs(10 * 10);
    payload.insert("iat".to_string(), now.as_secs().to_string());
    payload.insert("exp".to_string(), expiration.as_secs().to_string());
    payload.insert("iss".to_string(), app_id.to_string());
    jwt::encode(header, key_path.to_string(), payload)
}

fn http_get<T, U>(url: T, token: Option<U>) -> HubResult<hyper::client::response::Response>
where
    T: IntoUrl,
    U: ToString,
{
    let u = url.into_url().map_err(HubError::HttpClientParse)?;
    let endpoint = format!("{}://{}", u.scheme(), u.host_str().unwrap());
    let mut path = u.path();

    // ApiClient expects path to not have a leading slash, so if we detect one here (which is
    // almost always the case), then strip it off before passing it on. Failure to do so results in
    // URLs that look like https://api.github.com//meta, which obviously 404s.
    if path.starts_with("/") && path.len() > 1 {
        path = path.get(1..).unwrap();
    }

    let client = http_client(endpoint.as_str())?;
    let req = client.get_with_custom_url(path, |url| {
        if u.query().is_some() {
            url.set_query(Some(u.query().unwrap()))
        }
    });

    let req = req.header(Accept(vec![
        qitem(Mime(TopLevel::Application, SubLevel::Json, vec![])),
        qitem("application/vnd.github.v3+json".parse().unwrap()),
        qitem(
            "application/vnd.github.machine-man-preview+json"
                .parse()
                .unwrap(),
        ),
    ])).header(UserAgent(USER_AGENT.to_string()));
    let req = match token {
        Some(token) => req.header(Authorization(Bearer {
            token: token.to_string(),
        })),
        None => req,
    };
    req.send().map_err(HubError::HttpClient)
}

fn http_post<T, U>(url: T, token: Option<U>) -> HubResult<hyper::client::response::Response>
where
    T: IntoUrl,
    U: ToString,
{
    let u = url.into_url().map_err(HubError::HttpClientParse)?;
    let endpoint = format!("{}://{}", u.scheme(), u.host_str().unwrap());
    let mut path = u.path();

    if path.starts_with("/") && path.len() > 1 {
        path = path.get(1..).unwrap();
    }

    let client = http_client(endpoint.as_str())?;
    let req = client.post_with_custom_url(path, |url| {
        if u.query().is_some() {
            url.set_query(Some(u.query().unwrap()))
        }
    });

    let req = req.header(Accept(vec![
        qitem(Mime(TopLevel::Application, SubLevel::Json, vec![])),
        qitem("application/vnd.github.v3+json".parse().unwrap()),
        qitem(
            "application/vnd.github.machine-man-preview+json"
                .parse()
                .unwrap(),
        ),
    ])).header(UserAgent(USER_AGENT.to_string()));
    let req = match token {
        Some(token) => req.header(Authorization(Bearer {
            token: token.to_string(),
        })),
        None => req,
    };
    req.send().map_err(HubError::HttpClient)
}

fn http_client<T>(url: T) -> HubResult<ApiClient>
where
    T: IntoUrl,
{
    ApiClient::new(url, "bldr", "0.0.0", None).map_err(HubError::ApiClient)
}

#[cfg(test)]
mod tests {
    use super::*;
    use config;
    use std::env;

    #[test]
    fn use_a_proxy_from_the_env() {
        let proxy = env::var_os("HTTPS_PROXY");

        if proxy.is_some() {
            let p = proxy.unwrap();
            let pp = p.to_string_lossy();

            if !pp.is_empty() {
                let cfg = config::GitHubCfg::default();
                let client = GitHubClient::new(cfg);
                assert_eq!(client.meta().unwrap(), ());
            } else {
                assert!(true);
            }
        } else {
            assert!(true);
        }
    }
}
