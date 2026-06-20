#include "vpn_engine/protocol/aggregate_auth.hpp"

#include <algorithm>
#include <cctype>
#include <iterator>
#include <sstream>
#include <string_view>
#include <utility>

namespace exv {
namespace vpn_engine {
namespace protocol {
namespace {

constexpr std::size_t kMaxAggregateAuthResponseBytes = 1024 * 1024;

ValidationResult invalid(std::string code, std::string message) {
  ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

bool is_space(unsigned char ch) {
  return ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n';
}

std::string trim(std::string_view value) {
  while (!value.empty() && is_space(static_cast<unsigned char>(value.front())))
    value.remove_prefix(1);
  while (!value.empty() && is_space(static_cast<unsigned char>(value.back())))
    value.remove_suffix(1);
  return std::string(value);
}

std::string lower_ascii(std::string_view value) {
  std::string out;
  out.reserve(value.size());
  for (unsigned char ch : value)
    out.push_back(static_cast<char>(std::tolower(ch)));
  return out;
}

bool contains_ci(std::string_view haystack, std::string_view needle) {
  return lower_ascii(haystack).find(lower_ascii(needle)) != std::string::npos;
}

std::string xml_escape(std::string_view value) {
  std::string out;
  out.reserve(value.size());
  for (char ch : value) {
    switch (ch) {
    case '&':
      out += "&amp;";
      break;
    case '<':
      out += "&lt;";
      break;
    case '>':
      out += "&gt;";
      break;
    case '"':
      out += "&quot;";
      break;
    case '\'':
      out += "&apos;";
      break;
    default:
      out.push_back(ch);
      break;
    }
  }
  return out;
}

std::string xml_unescape(std::string_view value) {
  std::string out(value);
  const std::pair<const char *, const char *> entities[] = {
      {"&quot;", "\""}, {"&apos;", "'"}, {"&lt;", "<"},
      {"&gt;", ">"},     {"&amp;", "&"},
  };
  for (const auto &[from, to] : entities) {
    std::size_t pos = 0;
    while ((pos = out.find(from, pos)) != std::string::npos) {
      out.replace(pos, std::char_traits<char>::length(from), to);
      pos += std::char_traits<char>::length(to);
    }
  }
  return out;
}

std::string attr_value(std::string_view tag, std::string_view name) {
  const std::string key = std::string(name) + "=";
  std::size_t pos = tag.find(key);
  if (pos == std::string_view::npos)
    return {};
  pos += key.size();
  if (pos >= tag.size())
    return {};
  const char quote = tag[pos];
  if (quote != '"' && quote != '\'')
    return {};
  ++pos;
  const std::size_t end = tag.find(quote, pos);
  if (end == std::string_view::npos)
    return {};
  return xml_unescape(tag.substr(pos, end - pos));
}

std::string first_tag(std::string_view xml, std::string_view name) {
  const std::string open = "<" + std::string(name);
  const std::size_t start = xml.find(open);
  if (start == std::string_view::npos)
    return {};
  const std::size_t end = xml.find('>', start);
  if (end == std::string_view::npos)
    return {};
  return std::string(xml.substr(start, end - start + 1));
}

struct XmlElement {
  std::string tag;
  std::string body;
};

XmlElement first_element(std::string_view xml, std::string_view name) {
  const std::string open = "<" + std::string(name);
  const std::size_t start = xml.find(open);
  if (start == std::string_view::npos)
    return {};
  const std::size_t tag_end = xml.find('>', start);
  if (tag_end == std::string_view::npos)
    return {};

  XmlElement element;
  element.tag = std::string(xml.substr(start, tag_end - start + 1));
  const bool self_closing =
      element.tag.size() >= 2 && element.tag[element.tag.size() - 2] == '/';
  if (self_closing)
    return element;

  const std::string close = "</" + std::string(name) + ">";
  const std::size_t close_start = xml.find(close, tag_end + 1);
  if (close_start == std::string_view::npos)
    return element;
  element.body =
      std::string(xml.substr(tag_end + 1, close_start - tag_end - 1));
  return element;
}

std::string node_text(std::string_view xml, std::string_view name) {
  const std::string open = "<" + std::string(name);
  const std::string close = "</" + std::string(name) + ">";
  const std::size_t start = xml.find(open);
  if (start == std::string_view::npos)
    return {};
  const std::size_t open_end = xml.find('>', start);
  if (open_end == std::string_view::npos)
    return {};
  const std::size_t close_start = xml.find(close, open_end + 1);
  if (close_start == std::string_view::npos)
    return {};
  return xml_unescape(trim(xml.substr(open_end + 1, close_start - open_end - 1)));
}

std::vector<std::string> opaque_nodes(std::string_view xml) {
  std::vector<std::string> nodes;
  const std::string open = "<opaque";
  const std::string close = "</opaque>";
  std::size_t pos = 0;
  while ((pos = xml.find(open, pos)) != std::string_view::npos) {
    const std::size_t close_start = xml.find(close, pos);
    if (close_start == std::string_view::npos)
      break;
    const std::size_t end = close_start + close.size();
    nodes.emplace_back(xml.substr(pos, end - pos));
    pos = end;
  }
  return nodes;
}

std::vector<AggregateAuthField> input_fields(std::string_view xml) {
  std::vector<AggregateAuthField> fields;
  const std::string open = "<input";
  std::size_t pos = 0;
  while ((pos = xml.find(open, pos)) != std::string_view::npos) {
    const std::size_t tag_end = xml.find('>', pos);
    if (tag_end == std::string_view::npos)
      break;

    const std::string tag(xml.substr(pos, tag_end - pos + 1));
    AggregateAuthField field;
    field.name = attr_value(tag, "name");
    field.type = attr_value(tag, "type");
    field.label = attr_value(tag, "label");
    field.value = attr_value(tag, "value");

    const bool self_closing =
        tag.size() >= 2 && tag[tag.size() - 2] == '/';
    if (!self_closing) {
      const std::string close = "</input>";
      const std::size_t close_start = xml.find(close, tag_end + 1);
      if (close_start != std::string_view::npos && field.value.empty()) {
        field.value =
            xml_unescape(trim(xml.substr(tag_end + 1, close_start - tag_end - 1)));
        pos = close_start + close.size();
      } else {
        pos = tag_end + 1;
      }
    } else {
      pos = tag_end + 1;
    }

    if (!field.name.empty())
      fields.push_back(std::move(field));
  }
  return fields;
}

std::vector<AggregateAuthChoice> option_nodes(std::string_view select_body) {
  std::vector<AggregateAuthChoice> options;
  const std::string open = "<option";
  const std::string close = "</option>";
  std::size_t pos = 0;
  while ((pos = select_body.find(open, pos)) != std::string_view::npos) {
    const std::size_t tag_end = select_body.find('>', pos);
    if (tag_end == std::string_view::npos)
      break;
    const std::size_t close_start = select_body.find(close, tag_end + 1);
    if (close_start == std::string_view::npos)
      break;

    const std::string tag(select_body.substr(pos, tag_end - pos + 1));
    AggregateAuthChoice choice;
    const std::string explicit_value = attr_value(tag, "value");
    choice.label =
        xml_unescape(trim(select_body.substr(tag_end + 1, close_start - tag_end - 1)));
    choice.value = explicit_value.empty() ? choice.label : explicit_value;
    if (choice.label.empty())
      choice.label = choice.value;
    if (!choice.value.empty()) {
      const std::string selected = lower_ascii(attr_value(tag, "selected"));
      if (selected == "true" || selected == "selected")
        options.insert(options.begin(), std::move(choice));
      else
        options.push_back(std::move(choice));
    }
    pos = close_start + close.size();
  }
  return options;
}

std::vector<AggregateAuthField> select_fields(std::string_view xml) {
  std::vector<AggregateAuthField> fields;
  const std::string open = "<select";
  const std::string close = "</select>";
  std::size_t pos = 0;
  while ((pos = xml.find(open, pos)) != std::string_view::npos) {
    const std::size_t tag_end = xml.find('>', pos);
    if (tag_end == std::string_view::npos)
      break;
    const std::size_t close_start = xml.find(close, tag_end + 1);
    if (close_start == std::string_view::npos)
      break;

    const std::string tag(xml.substr(pos, tag_end - pos + 1));
    AggregateAuthField field;
    field.name = attr_value(tag, "name");
    field.type = "select";
    field.label = attr_value(tag, "label");
    const std::string_view body =
        xml.substr(tag_end + 1, close_start - tag_end - 1);
    field.options = option_nodes(body);
    if (!field.options.empty())
      field.value = field.options.front().value;
    if (!field.name.empty())
      fields.push_back(std::move(field));
    pos = close_start + close.size();
  }
  return fields;
}

bool has_field(const std::vector<AggregateAuthField> &fields,
               std::string_view name) {
  return std::any_of(fields.begin(), fields.end(),
                     [name](const AggregateAuthField &field) {
                       return field.name == name;
                     });
}

bool has_challenge_field(const std::vector<AggregateAuthField> &fields) {
  return std::any_of(fields.begin(), fields.end(),
                     [](const AggregateAuthField &field) {
                       const std::string name = lower_ascii(field.name);
                       return name.find("secondary") != std::string::npos ||
                              name.find("challenge") != std::string::npos ||
                              name.find("token") != std::string::npos ||
                              name.find("passcode") != std::string::npos;
                     });
}

std::string first_non_empty(std::string a, std::string b,
                            std::string c = {}, std::string d = {}) {
  if (!a.empty())
    return a;
  if (!b.empty())
    return b;
  if (!c.empty())
    return c;
  return d;
}

AggregateAuthHostScan host_scan_metadata(std::string_view xml) {
  AggregateAuthHostScan metadata;
  const XmlElement element = first_element(xml, "host-scan");
  if (element.tag.empty())
    return metadata;

  metadata.ticket = first_non_empty(attr_value(element.tag, "ticket"),
                                    node_text(element.body, "ticket"));
  metadata.token = first_non_empty(attr_value(element.tag, "token"),
                                   node_text(element.body, "token"));
  metadata.base_uri =
      first_non_empty(attr_value(element.tag, "base-uri"),
                      node_text(element.body, "base-uri"),
                      attr_value(element.tag, "base-url"),
                      node_text(element.body, "base-url"));
  metadata.wait_uri =
      first_non_empty(attr_value(element.tag, "wait-uri"),
                      node_text(element.body, "wait-uri"),
                      attr_value(element.tag, "wait-url"),
                      node_text(element.body, "wait-url"));
  return metadata;
}

void append_text_node(std::ostringstream &out, int indent,
                      const std::string &name, const std::string &value) {
  if (value.empty())
    return;
  out << std::string(static_cast<std::size_t>(indent), ' ') << "<" << name
      << ">" << xml_escape(value) << "</" << name << ">\n";
}

bool is_xml_name_start(unsigned char ch) {
  return std::isalpha(ch) || ch == '_' || ch == ':';
}

bool is_xml_name_char(unsigned char ch) {
  return is_xml_name_start(ch) || std::isdigit(ch) || ch == '-' || ch == '.';
}

bool is_safe_xml_name(std::string_view name) {
  if (name.empty() || !is_xml_name_start(static_cast<unsigned char>(name.front())))
    return false;
  for (unsigned char ch : name) {
    if (!is_xml_name_char(ch))
      return false;
  }
  return true;
}

std::string auth_reply_field_name(std::string_view name) {
  if (name.empty())
    return "password";
  if (name == "answer" || name == "whichpin" || name == "new_password")
    return "password";
  if (!is_safe_xml_name(name))
    return "password";
  return std::string(name);
}

} // namespace

std::string build_aggregate_auth_init_xml(
    const AggregateAuthInitRequest &request) {
  std::ostringstream out;
  out << "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
  out << "<config-auth client=\"vpn\" type=\"init\" "
         "aggregate-auth-version=\"2\">\n";
  out << "  <version who=\"vpn\">" << xml_escape(request.version)
      << "</version>\n";
  out << "  <device-id>" << xml_escape(request.device_id) << "</device-id>\n";
  out << "  <group-access>" << xml_escape(request.server_url)
      << "</group-access>\n";
  out << "  <capabilities>\n";
  out << "    <auth-method>single-sign-on-v2</auth-method>\n";
  out << "  </capabilities>\n";
  out << "</config-auth>";
  return out.str();
}

std::string build_aggregate_auth_reply_xml(
    const AggregateAuthReplyRequest &request) {
  std::ostringstream out;
  out << "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
  out << "<config-auth client=\"vpn\" type=\"auth-reply\" "
         "aggregate-auth-version=\"2\">\n";
  out << "  <version who=\"vpn\">" << xml_escape(request.version)
      << "</version>\n";
  out << "  <device-id>" << xml_escape(request.device_id) << "</device-id>\n";
  out << "  <capabilities>\n";
  out << "    <auth-method>single-sign-on-v2</auth-method>\n";
  out << "  </capabilities>\n";
  for (const std::string &opaque : request.opaque_xml)
    out << "  " << opaque << "\n";
  out << "  <auth>\n";
  append_text_node(out, 4, "username", request.username);
  append_text_node(out, 4, "password", request.password);
  if (!request.challenge_value.empty()) {
    append_text_node(out, 4,
                     auth_reply_field_name(request.challenge_field_name),
                     request.challenge_value);
  }
  out << "  </auth>\n";
  append_text_node(out, 2, "group-select", request.selected_group);
  out << "</config-auth>";
  return out.str();
}

ValidationResult parse_aggregate_auth_response(const std::string &xml,
                                               AggregateAuthResponse *out) {
  if (!out)
    return invalid("auth_response_invalid", "aggregate auth output is null");
  if (xml.size() > kMaxAggregateAuthResponseBytes) {
    return invalid("auth_response_too_large",
                   "aggregate auth response exceeds maximum size");
  }

  const std::string trimmed = trim(xml);
  if (trimmed.empty())
    return invalid("auth_response_invalid", "aggregate auth response is empty");
  if (contains_ci(trimmed, "<html")) {
    return invalid("auth_protocol_mismatch",
                   "server returned HTML instead of aggregate-auth XML");
  }
  if (!contains_ci(trimmed, "<config-auth")) {
    return invalid("auth_response_invalid",
                   "aggregate auth response missing config-auth root");
  }

  AggregateAuthResponse parsed;
  const std::string root = first_tag(trimmed, "config-auth");
  const std::string root_type = attr_value(root, "type");
  const std::string auth = first_tag(trimmed, "auth");
  parsed.auth_id = attr_value(auth, "id");
  parsed.message = node_text(trimmed, "message");
  parsed.fields = input_fields(trimmed);
  {
    std::vector<AggregateAuthField> selects = select_fields(trimmed);
    parsed.fields.insert(parsed.fields.end(),
                         std::make_move_iterator(selects.begin()),
                         std::make_move_iterator(selects.end()));
  }
  parsed.opaque_xml = opaque_nodes(trimmed);
  parsed.session_token = node_text(trimmed, "session-token");
  parsed.session_id = node_text(trimmed, "session-id");
  parsed.host_scan = host_scan_metadata(trimmed);

  if (!parsed.session_token.empty() || !parsed.session_id.empty()) {
    parsed.type = AggregateAuthResponseType::success;
  } else if (contains_ci(trimmed, "<host-scan")) {
    parsed.type = AggregateAuthResponseType::host_scan;
  } else if (parsed.auth_id == "error" || contains_ci(trimmed, "<error")) {
    parsed.type = AggregateAuthResponseType::error;
    parsed.error_code = "auth_rejected";
    parsed.error_message = node_text(trimmed, "error");
    if (parsed.error_message.empty())
      parsed.error_message = parsed.message;
  } else if (has_challenge_field(parsed.fields)) {
    parsed.type = AggregateAuthResponseType::challenge;
  } else if (has_field(parsed.fields, "group_list") &&
             !has_field(parsed.fields, "username") &&
             !has_field(parsed.fields, "password")) {
    parsed.type = AggregateAuthResponseType::group_select;
  } else if (parsed.auth_id == "success" || root_type == "complete") {
    parsed.type = AggregateAuthResponseType::success;
  } else {
    parsed.type = AggregateAuthResponseType::auth_request;
  }

  *out = std::move(parsed);
  return ValidationResult{};
}

} // namespace protocol
} // namespace vpn_engine
} // namespace exv
