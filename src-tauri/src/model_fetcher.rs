use std::collections::HashMap;
use serde_json::Value;

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

#[allow(dead_code)]
fn is_likely_ollama_url(base_url: &str) -> bool {
    let normalized = normalize_base_url(base_url).to_lowercase();
    normalized.contains("localhost")
        || normalized.contains("127.0.0.1")
        || normalized.contains("0.0.0.0")
}

struct Candidate {
    #[allow(dead_code)]
    label: String,
    url: String,
    headers: HashMap<String, String>,
}

fn build_endpoint_candidates(base_url: &str, auth_token: &str) -> Vec<Candidate> {
    let normalized = normalize_base_url(base_url);
    let mut candidates = Vec::new();

    let mut bearer_headers: HashMap<String, String> = HashMap::new();
    if !auth_token.is_empty() {
        bearer_headers.insert("Authorization".to_string(), format!("Bearer {}", auth_token));
    }

    // 1. OpenAI/new-api/ollama
    candidates.push(Candidate {
        label: "OpenAI/new-api/ollama".to_string(),
        url: format!("{}/v1/models", normalized),
        headers: bearer_headers.clone(),
    });

    if !auth_token.is_empty() {
        // 2. Anthropic
        let mut anthropic_headers = HashMap::new();
        anthropic_headers.insert("x-api-key".to_string(), auth_token.to_string());
        anthropic_headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        candidates.push(Candidate {
            label: "Anthropic".to_string(),
            url: format!("{}/v1/models", normalized),
            headers: anthropic_headers,
        });

        // 3. Gemini query key
        candidates.push(Candidate {
            label: "Gemini query key".to_string(),
            url: format!("{}/v1beta/models?key={}", normalized, auth_token),
            headers: HashMap::new(),
        });

        // 4. Gemini header key
        let mut gemini_headers = HashMap::new();
        gemini_headers.insert("x-goog-api-key".to_string(), auth_token.to_string());
        candidates.push(Candidate {
            label: "Gemini header key".to_string(),
            url: format!("{}/v1beta/models", normalized),
            headers: gemini_headers,
        });
    }

    // Check if path is non-empty (i.e., base_url has a path component beyond root)
    if let Ok(parsed) = url::Url::parse(&normalized) {
        let path = parsed.path().trim_end_matches('/');
        if !path.is_empty() {
            let root_url = format!(
                "{}://{}",
                parsed.scheme(),
                parsed.host_str().unwrap_or("")
            );
            let root_url = if let Some(port) = parsed.port() {
                format!("{}:{}", root_url, port)
            } else {
                root_url
            };

            // 5. Root-path OpenAI
            candidates.push(Candidate {
                label: format!("OpenAI (root {})", root_url),
                url: format!("{}/v1/models", root_url),
                headers: bearer_headers.clone(),
            });

            if !auth_token.is_empty() {
                // 6. Root-path Anthropic
                let mut anthropic_headers = HashMap::new();
                anthropic_headers.insert("x-api-key".to_string(), auth_token.to_string());
                anthropic_headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
                candidates.push(Candidate {
                    label: format!("Anthropic (root {})", root_url),
                    url: format!("{}/v1/models", root_url),
                    headers: anthropic_headers,
                });
            }
        }
    }

    candidates
}

fn extract_model_ids(payload: &Value) -> Vec<String> {
    let mut models = Vec::new();

    if let Some(obj) = payload.as_object() {
        // Handle data[].id or data[].display_name
        if let Some(Value::Array(data)) = obj.get("data") {
            for item in data {
                if let Some(item_obj) = item.as_object() {
                    let model_id = item_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            item_obj
                                .get("display_name")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                        });
                    if let Some(id) = model_id {
                        models.push(id.to_string());
                    }
                }
            }
        }

        // Handle models[].name or models[].display_name or models[].id
        if let Some(Value::Array(model_list)) = obj.get("models") {
            for item in model_list {
                if let Some(item_obj) = item.as_object() {
                    let model_name = item_obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            item_obj
                                .get("display_name")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                        })
                        .or_else(|| {
                            item_obj
                                .get("id")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                        });
                    if let Some(name) = model_name {
                        models.push(name.to_string());
                    }
                }
            }
        }
    }

    // Deduplicate preserving order, then sort
    let mut seen = std::collections::HashSet::new();
    models.retain(|m| seen.insert(m.clone()));
    models.sort();
    models
}

#[tauri::command]
pub async fn fetch_claude_models(
    base_url: String,
    auth_token: String,
) -> Result<Vec<String>, String> {
    let candidates = build_endpoint_candidates(&base_url, &auth_token);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .no_proxy()
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    for candidate in &candidates {
        let mut req = client.get(&candidate.url);
        for (k, v) in &candidate.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<Value>().await {
                    Ok(payload) => {
                        let ids = extract_model_ids(&payload);
                        if !ids.is_empty() {
                            return Ok(ids);
                        }
                    }
                    Err(_) => continue,
                }
            }
            _ => continue,
        }
    }

    Err("No model list could be retrieved from any endpoint".to_string())
}
