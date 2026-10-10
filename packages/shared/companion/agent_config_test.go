package companion

import "testing"

func TestAgentServiceSettingsBracketedHost(t *testing.T) {
	for _, endpoint := range []string{"http://[::1]:80", "http://[::ffff:127.0.0.1]:80", "HTTP://[::1]:80/a[[]", "http://127.0.0.1:80/a[[]"} {
		t.Run(endpoint, func(t *testing.T) {
			if err := (AgentServiceSettings{Endpoint: endpoint, APIKeyEnv: "KEY"}).Validate(); err != nil {
				t.Fatalf("valid loopback endpoint: %v", err)
			}
		})
	}
	for _, endpoint := range []string{"http://x[::1]:80", "http://abc[::1]:80/agent", "http://[[::1]:80"} {
		t.Run(endpoint, func(t *testing.T) {
			if err := (AgentServiceSettings{Endpoint: endpoint, APIKeyEnv: "KEY"}).Validate(); err == nil {
				t.Fatal("malformed bracketed host accepted")
			}
		})
	}
}
