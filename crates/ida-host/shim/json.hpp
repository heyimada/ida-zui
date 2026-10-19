// Minimal JSON writer for shim replies. Strings are sanitised to valid UTF-8
// (IDA names are usually UTF-8 but nothing guarantees it) and control bytes,
// including IDA's color tags, are emitted as \u00XX escapes.
#pragma once

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

class Json
{
public:
  std::string s;

  Json &obj() { value(); s += '{'; first_.push_back(true); return *this; }
  Json &end_obj() { s += '}'; first_.pop_back(); return *this; }
  Json &arr() { value(); s += '['; first_.push_back(true); return *this; }
  Json &end_arr() { s += ']'; first_.pop_back(); return *this; }

  Json &key(const char *k)
  {
    value();
    write_string(k, strlen(k));
    s += ':';
    after_key_ = true;
    return *this;
  }

  Json &str(const char *v) { return str(v, v != nullptr ? strlen(v) : 0); }
  Json &str(const char *v, size_t n) { value(); write_string(v, n); return *this; }
  Json &str(const std::string &v) { return str(v.data(), v.size()); }

  Json &u64(uint64_t v)
  {
    value();
    char buf[32];
    qsnprintf(buf, sizeof(buf), "%llu", (unsigned long long)v);
    s += buf;
    return *this;
  }

  Json &i64(int64_t v)
  {
    value();
    char buf[32];
    qsnprintf(buf, sizeof(buf), "%lld", (long long)v);
    s += buf;
    return *this;
  }

  Json &boolean(bool v) { value(); s += v ? "true" : "false"; return *this; }
  /// Inserts pre-built JSON as a value.
  Json &raw(const std::string &v) { value(); s += v; return *this; }
  Json &null() { value(); s += "null"; return *this; }

  /// Emits `v`, or null when `v == none`.
  Json &opt_u64(uint64_t v, uint64_t none) { return v == none ? null() : u64(v); }

  Json &kv(const char *k, const char *v) { return key(k).str(v); }
  Json &kv(const char *k, const std::string &v) { return key(k).str(v); }
  Json &kv(const char *k, uint64_t v) { return key(k).u64(v); }
  Json &kvi(const char *k, int64_t v) { return key(k).i64(v); }
  Json &kvb(const char *k, bool v) { return key(k).boolean(v); }

private:
  std::vector<bool> first_;
  bool after_key_ = false;

  void value()
  {
    if ( after_key_ )
    {
      after_key_ = false;
      return;
    }
    if ( !first_.empty() )
    {
      if ( !first_.back() )
        s += ',';
      first_.back() = false;
    }
  }

  static size_t utf8_len(const unsigned char *p, size_t avail)
  {
    unsigned char c = p[0];
    size_t n = c < 0x80 ? 1 : (c >> 5) == 0x6 ? 2 : (c >> 4) == 0xE ? 3 : (c >> 3) == 0x1E ? 4 : 0;
    if ( n == 0 || n > avail )
      return 0;
    for ( size_t i = 1; i < n; i++ )
      if ( (p[i] & 0xC0) != 0x80 )
        return 0;
    return n;
  }

  void write_string(const char *v, size_t n)
  {
    s += '"';
    const unsigned char *p = (const unsigned char *)v;
    size_t i = 0;
    while ( i < n )
    {
      unsigned char c = p[i];
      if ( c == '"' ) { s += "\\\""; i++; continue; }
      if ( c == '\\' ) { s += "\\\\"; i++; continue; }
      if ( c < 0x20 )
      {
        char buf[8];
        qsnprintf(buf, sizeof(buf), "\\u%04x", c);
        s += buf;
        i++;
        continue;
      }
      size_t len = utf8_len(p + i, n - i);
      if ( len == 0 )
      {
        s += "\xEF\xBF\xBD"; // U+FFFD
        i++;
        continue;
      }
      s.append(v + i, len);
      i += len;
    }
    s += '"';
  }
};
