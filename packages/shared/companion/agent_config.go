package companion

import (
	"errors"
	"fmt"
	"net"
	"net/url"
	"strconv"
	"strings"
)

// AgentServiceSettings 是独立伙伴 Agent 服务的静态连接配置。密钥只在调用方
// 从 `APIKeyEnv` 读取，设置本身从不保存密钥值。
type AgentServiceSettings struct {
	Endpoint  string `json:"endpoint"`
	APIKeyEnv string `json:"apiKeyEnv"`
}

// Validate 校验 Agent 服务只能通过明确的本机明文地址访问，避免凭据经由 DNS、
// 重定向或 URL 附加部分离开进程边界。
func (s AgentServiceSettings) Validate() error {
	if s.Endpoint == "" {
		return errors.New("companion: agentService 缺少 endpoint")
	}
	parsed, err := url.Parse(s.Endpoint)
	if err != nil {
		return fmt.Errorf("companion: 解析 agentService endpoint: %w", err)
	}
	// Older Go parsers discard host text before the last opening bracket.
	_, authority, _ := strings.Cut(s.Endpoint, "://")
	if end := strings.IndexAny(authority, "/?#"); end >= 0 {
		authority = authority[:end]
	}
	if strings.LastIndexByte(authority, '[') > 0 {
		return errors.New("companion: agentService endpoint has an invalid bracketed host")
	}
	if parsed.Scheme != "http" {
		return errors.New("companion: agentService endpoint 必须是 http URL")
	}
	if parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" {
		return errors.New("companion: agentService endpoint 不得携带 userinfo、query 或 fragment")
	}
	if port := parsed.Port(); port != "" {
		value, parseErr := strconv.ParseUint(port, 10, 16)
		if parseErr != nil || value == 0 {
			return errors.New("companion: agentService endpoint port 必须位于 1..65535")
		}
	}
	ip := net.ParseIP(parsed.Hostname())
	if ip == nil || !ip.IsLoopback() {
		return errors.New("companion: agentService endpoint host 必须是 loopback IP 字面量")
	}
	if s.APIKeyEnv == "" {
		return errors.New("companion: agentService 缺少 apiKeyEnv")
	}
	return nil
}
