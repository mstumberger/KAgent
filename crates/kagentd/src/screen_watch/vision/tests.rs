use super::*;

#[test]
fn local_is_decided_by_the_address_of_the_provider() {
    for url in ["http://127.0.0.1:11434", "http://127.0.0.1:11434/v1", "http://localhost:11434/v1", "http://192.168.1.20:1234/v1", "http://10.0.0.5:8080", "http://172.16.4.2:11434", "http://[::1]:11434", "http://[fd12::1]:11434", "http://169.254.1.1:11434", "http://ollama.localhost:11434"] {
        assert!(is_local_url(url), "{url}");
    }
    for url in ["https://ollama.com/v1", "https://generativelanguage.googleapis.com/v1beta/openai", "http://8.8.8.8:11434", "http://172.32.0.1:11434", "http://box.lan:11434", "http://localhost.evil.example/v1", "http://127.0.0.1.evil.example", "", "not a url", "http://[2001:db8::1]:11434"] {
        assert!(!is_local_url(url), "{url}");
    }
}

#[test]
fn the_same_server_and_model_is_one_model() {
    let m = LocalModel { id: "gemma3:4b".into(), base: "http://127.0.0.1:11434/v1".into(), api: Api::Ollama };
    assert!(m.same_as("http://127.0.0.1:11434", "gemma3:4b"));
    assert!(!m.same_as("http://127.0.0.1:11434", "gemma3:12b"));
    assert!(!m.same_as("http://10.0.0.2:11434", "gemma3:4b"));
}
