package archcheck_test

import (
	"bytes"
	"fmt"
	"go/parser"
	"go/token"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"unicode"
	"unicode/utf8"
)

type commentLanguage string

const (
	commentLanguageGo         commentLanguage = "Go"
	commentLanguageRust       commentLanguage = "Rust"
	commentLanguageC          commentLanguage = "C"
	commentLanguageGDScript   commentLanguage = "GDScript"
	commentLanguageJavaScript commentLanguage = "JavaScript"
	commentLanguageTypeScript commentLanguage = "TypeScript"
)

type commentScanSource struct {
	path string
	data []byte
}

type sourceCommentToken struct {
	offset int
	text   []byte
}

type codeCommentLanguageFinding struct {
	path string
	line int
	text string
}

var firstPartyCommentSourceRoots = []string{"apps", "packages", "scripts"}

// Each excluded directory denotes a generated, vendored, or copied-license tree.
var commentSourceExcludedDirectories = map[string]string{
	".godot":       "generated Godot cache",
	".venv":        "vendored Python environment",
	"dist":         "generated distribution output",
	"generated":    "generated source output",
	"licenses":     "copied license material",
	"node_modules": "vendored JavaScript dependencies",
	"target":       "generated Rust output",
	"third-party":  "vendored or copied-license material",
	"third_party":  "vendored or copied-license material",
	"vendor":       "vendored source",
}

func scanCodeCommentLanguage(sources []commentScanSource) ([]codeCommentLanguageFinding, error) {
	sources = slices.Clone(sources)
	slices.SortFunc(sources, func(left, right commentScanSource) int {
		return strings.Compare(left.path, right.path)
	})

	var findings []codeCommentLanguageFinding
	for _, source := range sources {
		comments, err := sourceCommentTokens(source)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", source.path, err)
		}
		for _, comment := range comments {
			startLine := bytes.Count(source.data[:comment.offset], []byte{'\n'}) + 1
			for lineOffset, line := range bytes.Split(comment.text, []byte{'\n'}) {
				if !containsHan(line) {
					continue
				}
				findings = append(findings, codeCommentLanguageFinding{
					path: source.path,
					line: startLine + lineOffset,
					text: strings.TrimSpace(string(line)),
				})
			}
		}
	}
	return findings, nil
}

func containsHan(text []byte) bool {
	for _, character := range string(text) {
		if unicode.Is(unicode.Han, character) {
			return true
		}
	}
	return false
}

func sourceCommentTokens(source commentScanSource) ([]sourceCommentToken, error) {
	language, ok := commentLanguageForPath(source.path)
	if !ok {
		return nil, fmt.Errorf("unsupported source type")
	}
	switch language {
	case commentLanguageGo:
		return goCommentTokens(source)
	case commentLanguageRust:
		return rustCommentTokens(source.data)
	case commentLanguageC:
		return cCommentTokens(source.data)
	case commentLanguageGDScript:
		return gdscriptCommentTokens(source.data)
	case commentLanguageJavaScript, commentLanguageTypeScript:
		return javascriptCommentTokens(source.data)
	default:
		return nil, fmt.Errorf("unsupported source language %q", language)
	}
}

func commentLanguageForPath(path string) (commentLanguage, bool) {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".go":
		return commentLanguageGo, true
	case ".rs":
		return commentLanguageRust, true
	case ".c", ".h":
		return commentLanguageC, true
	case ".gd":
		return commentLanguageGDScript, true
	case ".js", ".jsx", ".mjs", ".cjs":
		return commentLanguageJavaScript, true
	case ".ts", ".tsx", ".mts", ".cts":
		return commentLanguageTypeScript, true
	default:
		return "", false
	}
}

func goCommentTokens(source commentScanSource) ([]sourceCommentToken, error) {
	files := token.NewFileSet()
	parsed, err := parser.ParseFile(files, source.path, source.data, parser.ParseComments|parser.SkipObjectResolution)
	if err != nil {
		return nil, err
	}
	var comments []sourceCommentToken
	for _, group := range parsed.Comments {
		for _, comment := range group.List {
			position := files.Position(comment.Pos())
			comments = append(comments, sourceCommentToken{offset: position.Offset, text: []byte(comment.Text)})
		}
	}
	return comments, nil
}

func rustCommentTokens(source []byte) ([]sourceCommentToken, error) {
	var comments []sourceCommentToken
	for index := 0; index < len(source); {
		if end, ok, err := rustRawLiteralEnd(source, index); ok {
			if err != nil {
				return nil, err
			}
			index = end
			continue
		}
		if source[index] == 'b' || source[index] == 'c' {
			if index+1 < len(source) && source[index+1] == '"' {
				end, err := quotedLiteralEnd(source, index+1, source[index+1])
				if err != nil {
					return nil, err
				}
				index = end
				continue
			}
			if source[index] == 'b' && index+1 < len(source) && source[index+1] == '\'' {
				if end, ok := rustCharacterLiteralEnd(source, index+1); ok {
					index = end
					continue
				}
			}
		}
		switch source[index] {
		case '"':
			end, err := quotedLiteralEnd(source, index, source[index])
			if err != nil {
				return nil, err
			}
			index = end
		case '\'':
			if end, ok := rustCharacterLiteralEnd(source, index); ok {
				index = end
			} else {
				index++
			}
		case '/':
			comment, end, ok, err := slashCommentAt(source, index, true, false)
			if err != nil {
				return nil, err
			}
			if ok {
				comments = append(comments, comment)
				index = end
			} else {
				index++
			}
		default:
			index++
		}
	}
	return comments, nil
}

func cCommentTokens(source []byte) ([]sourceCommentToken, error) {
	var comments []sourceCommentToken
	for index := 0; index < len(source); {
		switch source[index] {
		case '"', '\'':
			end, err := quotedLiteralEnd(source, index, source[index])
			if err != nil {
				return nil, err
			}
			index = end
		case '/':
			comment, end, ok, err := slashCommentAt(source, index, false, true)
			if err != nil {
				return nil, err
			}
			if ok {
				comments = append(comments, comment)
				index = end
			} else {
				index++
			}
		default:
			index++
		}
	}
	return comments, nil
}

func gdscriptCommentTokens(source []byte) ([]sourceCommentToken, error) {
	var comments []sourceCommentToken
	for index := 0; index < len(source); {
		if source[index] == 'r' && index+1 < len(source) && (source[index+1] == '"' || source[index+1] == '\'') {
			end, err := gdscriptStringEnd(source, index+1, true)
			if err != nil {
				return nil, err
			}
			index = end
			continue
		}
		switch source[index] {
		case '"', '\'':
			end, err := gdscriptStringEnd(source, index, false)
			if err != nil {
				return nil, err
			}
			index = end
		case '#':
			end := lineCommentEnd(source, index, false)
			comments = append(comments, sourceCommentToken{offset: index, text: source[index:end]})
			index = end
		default:
			index++
		}
	}
	return comments, nil
}

func javascriptCommentTokens(source []byte) ([]sourceCommentToken, error) {
	comments, end, err := javascriptCommentsInRange(source, 0, false)
	if err != nil {
		return nil, err
	}
	if end != len(source) {
		return nil, fmt.Errorf("scanner stopped at offset %d", end)
	}
	return comments, nil
}

func javascriptCommentsInRange(source []byte, start int, stopAtRightBrace bool) ([]sourceCommentToken, int, error) {
	var comments []sourceCommentToken
	braceDepth := 0
	for index := start; index < len(source); {
		switch source[index] {
		case '"', '\'':
			end, err := quotedLiteralEnd(source, index, source[index])
			if err != nil {
				return nil, 0, err
			}
			index = end
		case '`':
			nested, end, err := javascriptTemplateEnd(source, index)
			if err != nil {
				return nil, 0, err
			}
			comments = append(comments, nested...)
			index = end
		case '/':
			comment, end, ok, err := slashCommentAt(source, index, false, false)
			if err != nil {
				return nil, 0, err
			}
			if ok {
				comments = append(comments, comment)
				index = end
				continue
			}
			if javascriptRegexCanStart(source, index) {
				end, err := javascriptRegexEnd(source, index)
				if err != nil {
					return nil, 0, err
				}
				index = end
				continue
			}
			index++
		case '{':
			if stopAtRightBrace {
				braceDepth++
			}
			index++
		case '}':
			if !stopAtRightBrace {
				index++
				continue
			}
			if braceDepth == 0 {
				return comments, index + 1, nil
			}
			braceDepth--
			index++
		default:
			index++
		}
	}
	if stopAtRightBrace {
		return nil, 0, fmt.Errorf("unterminated template expression at offset %d", start)
	}
	return comments, len(source), nil
}

func javascriptTemplateEnd(source []byte, start int) ([]sourceCommentToken, int, error) {
	var comments []sourceCommentToken
	for index := start + 1; index < len(source); index++ {
		switch source[index] {
		case '\\':
			index++
		case '`':
			return comments, index + 1, nil
		case '$':
			if index+1 >= len(source) || source[index+1] != '{' {
				continue
			}
			nested, end, err := javascriptCommentsInRange(source, index+2, true)
			if err != nil {
				return nil, 0, err
			}
			comments = append(comments, nested...)
			index = end - 1
		}
	}
	return nil, 0, fmt.Errorf("unterminated template literal at offset %d", start)
}

func slashCommentAt(source []byte, start int, nestedBlock, spliceLine bool) (sourceCommentToken, int, bool, error) {
	if start+1 >= len(source) || source[start] != '/' {
		return sourceCommentToken{}, start, false, nil
	}
	switch source[start+1] {
	case '/':
		end := lineCommentEnd(source, start, spliceLine)
		return sourceCommentToken{offset: start, text: source[start:end]}, end, true, nil
	case '*':
		end, err := blockCommentEnd(source, start, nestedBlock)
		if err != nil {
			return sourceCommentToken{}, 0, false, err
		}
		return sourceCommentToken{offset: start, text: source[start:end]}, end, true, nil
	default:
		return sourceCommentToken{}, start, false, nil
	}
}

func lineCommentEnd(source []byte, start int, spliceLine bool) int {
	for index := start + 2; index < len(source); index++ {
		if source[index] != '\n' && source[index] != '\r' {
			continue
		}
		if spliceLine && hasOddBackslashRun(source, index) {
			if source[index] == '\r' && index+1 < len(source) && source[index+1] == '\n' {
				index++
			}
			continue
		}
		return index
	}
	return len(source)
}

func hasOddBackslashRun(source []byte, end int) bool {
	count := 0
	for index := end - 1; index >= 0 && source[index] == '\\'; index-- {
		count++
	}
	return count%2 == 1
}

func blockCommentEnd(source []byte, start int, nested bool) (int, error) {
	depth := 1
	for index := start + 2; index+1 < len(source); index++ {
		pair := string(source[index : index+2])
		if nested && pair == "/*" {
			depth++
			index++
			continue
		}
		if pair != "*/" {
			continue
		}
		depth--
		index++
		if depth == 0 {
			return index + 1, nil
		}
	}
	return 0, fmt.Errorf("unterminated block comment at offset %d", start)
}

func quotedLiteralEnd(source []byte, start int, quote byte) (int, error) {
	for index := start + 1; index < len(source); index++ {
		switch source[index] {
		case '\\':
			index++
		case quote:
			return index + 1, nil
		}
	}
	return 0, fmt.Errorf("unterminated quoted literal at offset %d", start)
}

func gdscriptStringEnd(source []byte, start int, raw bool) (int, error) {
	quote := source[start]
	triple := start+2 < len(source) && source[start+1] == quote && source[start+2] == quote
	if !triple {
		if raw {
			for index := start + 1; index < len(source); index++ {
				if source[index] == quote {
					return index + 1, nil
				}
			}
			return 0, fmt.Errorf("unterminated raw GDScript string at offset %d", start)
		}
		return quotedLiteralEnd(source, start, quote)
	}
	for index := start + 3; index+2 < len(source); index++ {
		if !raw && source[index] == '\\' {
			index++
			continue
		}
		if source[index] == quote && source[index+1] == quote && source[index+2] == quote {
			return index + 3, nil
		}
	}
	return 0, fmt.Errorf("unterminated triple-quoted GDScript string at offset %d", start)
}

func rustRawLiteralEnd(source []byte, start int) (end int, ok bool, err error) {
	index := start
	if index < len(source) && (source[index] == 'b' || source[index] == 'c') {
		index++
	}
	if index >= len(source) || source[index] != 'r' {
		return 0, false, nil
	}
	index++
	hashes := 0
	for index < len(source) && source[index] == '#' {
		hashes++
		index++
	}
	if index >= len(source) || source[index] != '"' {
		return 0, false, nil
	}
	for index++; index < len(source); index++ {
		if source[index] != '"' || index+hashes >= len(source) {
			continue
		}
		matched := true
		for hash := 0; hash < hashes; hash++ {
			if source[index+1+hash] != '#' {
				matched = false
				break
			}
		}
		if matched {
			return index + 1 + hashes, true, nil
		}
	}
	return 0, true, fmt.Errorf("unterminated Rust raw string at offset %d", start)
}

func rustCharacterLiteralEnd(source []byte, start int) (int, bool) {
	index := start + 1
	if index >= len(source) {
		return 0, false
	}
	if source[index] == '\\' {
		index++
		if index >= len(source) {
			return 0, false
		}
		switch source[index] {
		case 'x':
			index += 3
		case 'u':
			index += 2
			for index < len(source) && source[index] != '}' {
				index++
			}
			index++
		default:
			index++
		}
	} else {
		_, size := utf8.DecodeRune(source[index:])
		index += size
	}
	if index < len(source) && source[index] == '\'' {
		return index + 1, true
	}
	return 0, false
}

func javascriptRegexCanStart(source []byte, slash int) bool {
	index := slash - 1
	for index >= 0 && isASCIIWhitespace(source[index]) {
		index--
	}
	if index < 0 {
		return true
	}
	if strings.ContainsRune("([{=,:;!?&|~%^>*+-", rune(source[index])) {
		return true
	}
	if !isJavaScriptIdentifierPart(source[index]) {
		return false
	}
	end := index + 1
	for index >= 0 && isJavaScriptIdentifierPart(source[index]) {
		index--
	}
	switch string(source[index+1 : end]) {
	case "await", "case", "delete", "in", "instanceof", "new", "of", "return", "throw", "typeof", "void", "yield":
		return true
	default:
		return false
	}
}

func javascriptRegexEnd(source []byte, start int) (int, error) {
	inClass := false
	for index := start + 1; index < len(source); index++ {
		switch source[index] {
		case '\\':
			index++
		case '[':
			inClass = true
		case ']':
			inClass = false
		case '/':
			if inClass {
				continue
			}
			index++
			for index < len(source) && isJavaScriptIdentifierPart(source[index]) {
				index++
			}
			return index, nil
		case '\n', '\r':
			return 0, fmt.Errorf("unterminated JavaScript regular expression at offset %d", start)
		}
	}
	return 0, fmt.Errorf("unterminated JavaScript regular expression at offset %d", start)
}

func isASCIIWhitespace(character byte) bool {
	return character == ' ' || character == '\t' || character == '\n' || character == '\r' || character == '\f'
}

func isJavaScriptIdentifierPart(character byte) bool {
	return character >= 'a' && character <= 'z' ||
		character >= 'A' && character <= 'Z' ||
		character >= '0' && character <= '9' ||
		character == '_' || character == '$'
}

func collectFirstPartyCommentSources(root string) ([]commentScanSource, error) {
	var sources []commentScanSource
	for _, sourceRoot := range firstPartyCommentSourceRoots {
		absoluteRoot := filepath.Join(root, sourceRoot)
		if _, err := os.Stat(absoluteRoot); err != nil {
			if os.IsNotExist(err) {
				continue
			}
			return nil, err
		}
		err := filepath.WalkDir(absoluteRoot, func(path string, entry fs.DirEntry, walkErr error) error {
			if walkErr != nil {
				return walkErr
			}
			relative, err := filepath.Rel(root, path)
			if err != nil {
				return err
			}
			relative = filepath.ToSlash(relative)
			if entry.IsDir() {
				// The ignored Py4Godot runtime is verified upstream material.
				if relative == "apps/mornlea-godot/addons/py4godot" {
					return fs.SkipDir
				}
				if _, excluded := commentSourceExcludedDirectories[entry.Name()]; excluded {
					return fs.SkipDir
				}
				return nil
			}
			if isCopiedLicenseFile(entry.Name()) {
				return nil
			}
			if _, ok := commentLanguageForPath(relative); !ok {
				return nil
			}
			data, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			sources = append(sources, commentScanSource{path: relative, data: data})
			return nil
		})
		if err != nil {
			return nil, err
		}
	}
	slices.SortFunc(sources, func(left, right commentScanSource) int {
		return strings.Compare(left.path, right.path)
	})
	return sources, nil
}

func isCopiedLicenseFile(name string) bool {
	name = strings.ToLower(name)
	return name == "license" || strings.HasPrefix(name, "license.") ||
		name == "copying" || strings.HasPrefix(name, "copying.")
}
