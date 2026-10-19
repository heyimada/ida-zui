// Local types, structures, unions, enumerations and stack frames. Types are
// addressed by name, members by their index in the type's member vector (the
// UI refetches after every edit, so an index never outlives its listing).
// Units: struct offsets and sizes cross the protocol in bytes (bitfields add
// a bit offset and width); frame offsets are offsets into the frame struct.

#include <pro.h>
#include <ida.hpp>
#include <idp.hpp>
#include <funcs.hpp>
#include <frame.hpp>
#include <kernwin.hpp>
#include <name.hpp>
#include <typeinf.hpp>
#include <xref.hpp>
#include <hexrays.hpp>

#include <string>

#include "common.hpp"

//--------------------------------------------------------------------------
// Helpers

static std::string g_pmsg;

// parse_decls reports errors through a printer; collect them instead of
// letting them reach IDA's message window.
static int idaapi collect_printer(const char *fmt, ...)
{
  va_list va;
  va_start(va, fmt);
  qstring q;
  q.cat_vsprnt(fmt, va);
  va_end(va);
  g_pmsg.append(q.c_str(), q.length());
  return (int)q.length();
}

static const char *kind_name(const tinfo_t &t)
{
  return t.is_typedef() ? "typedef"
       : t.is_struct() ? "struct"
       : t.is_union() ? "union"
       : t.is_enum() ? "enum"
       : t.is_func() ? "func"
       : "other";
}

// Types may have changed under cached pseudocode.
static void types_changed()
{
  if ( g_hexrays )
    clear_cached_cfuncs();
}

static const char *code_str(tinfo_code_t rc)
{
  return tinfo_errstr(rc);
}

// Loads the type called `name` as stored in its ordinal (a typedef stays a
// typedef).
static bool load_named(const char *name, tinfo_t *t)
{
  return t->get_named_type(nullptr, name, BTF_TYPEDEF, true);
}

// Like load_named, but follows typedefs to the struct, union or enum.
static bool load_final(const char *name, tinfo_t *t)
{
  if ( !load_named(name, t) )
    return false;
  qstring fin;
  if ( t->is_typeref() && t->get_final_type_name(&fin) )
    return t->get_named_type(nullptr, fin.c_str(), BTF_TYPEDEF, true);
  return true;
}

// Parses a type such as "int", "char [16]", "char *[4]" or "int (*)(int)".
static bool parse_type_text(const char *decl, tinfo_t *out)
{
  qstring text = decl;
  text.trim2();
  if ( text.empty() )
    return false;
  size_t fp = text.find("(*)");
  size_t br = text.find('[');
  if ( fp != qstring::npos )
    text.replace("(*)", "(* __izs_tmp)");
  else if ( br != qstring::npos )
    text.insert(br, " __izs_tmp ");
  else
    text.append(" __izs_tmp");
  text.append(';');
  qstring name;
  return parse_decl(out, &name, nullptr, text.c_str(), PT_SIL);
}

static void print_type_text(qstring *out, const tinfo_t &t)
{
  t.print(out, nullptr, PRTYPE_1LINE);
}

//--------------------------------------------------------------------------
// Local types

extern "C" {

int izs_type_list(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  uint32 limit = get_ordinal_limit(nullptr);
  for ( uint32 ord = 1; ord < limit; ord++ )
  {
    const char *name = get_numbered_type_name(nullptr, ord);
    if ( name == nullptr )
      continue;
    tinfo_t t;
    if ( !t.get_numbered_type(nullptr, ord, BTF_TYPEDEF, true) )
      continue;
    size_t size = t.get_size();
    uint64_t members = 0;
    if ( t.is_typedef() )
      members = 0;
    else if ( t.is_udt() )
      members = (uint64_t)t.get_udt_nmembers();
    else if ( t.is_enum() )
      members = (uint64_t)t.get_enum_nmembers();
    j.obj()
      .kv("ordinal", (uint64_t)ord)
      .kv("name", name)
      .kv("kind", kind_name(t))
      .key("size").opt_u64(size, BADSIZE)
      .kv("members", members)
      .end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_type_decl(const char *name)
{
  REQUIRE_DB();
  GUARD_BEGIN
  tinfo_t t;
  if ( !load_named(name, &t) )
    return fail("no local type named %s", name);
  qstring decl;
  t.print(&decl, name, PRTYPE_MULTI | PRTYPE_TYPE | PRTYPE_DEF | PRTYPE_SEMI);
  Json j;
  j.obj()
    .kv("name", name)
    .kv("ordinal", (uint64_t)get_type_ordinal(nullptr, name))
    .kv("kind", kind_name(t))
    .key("size").opt_u64(t.get_size(), BADSIZE)
    .kv("decl", qs(decl))
    .end_obj();
  return ok(j);
  GUARD_END
}

// Parses declarations (any number) into the local types, one statement at a
// time like IDA's own Local Types window: a statement with errors stores
// nothing, the others stay. A declaration whose name already exists replaces
// that type. The parser's messages come back verbatim.
int izs_type_apply(const char *decl)
{
  REQUIRE_DB();
  GUARD_BEGIN
  g_pmsg.clear();
  uint32 before = get_ordinal_limit(nullptr);
  undo_point();
  int errors = parse_decls(nullptr, decl, collect_printer, HTI_DCL | HTI_SEMICOLON);
  Json names;
  names.arr();
  uint32 after = get_ordinal_limit(nullptr);
  for ( uint32 ord = before; ord < after; ord++ )
  {
    const char *n = get_numbered_type_name(nullptr, ord);
    if ( n != nullptr )
      names.str(n);
  }
  names.end_arr();
  types_changed();
  Json j;
  j.obj()
    .kvb("applied", errors == 0)
    .kv("errors", (uint64_t)errors)
    .kv("messages", g_pmsg);
  j.key("names").raw(names.s);
  j.end_obj();
  return ok(j);
  GUARD_END
}

int izs_type_create(const char *name, const char *kind)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( get_type_ordinal(nullptr, name) != 0 )
    return fail("a type named %s already exists", name);
  tinfo_t t;
  bool made;
  if ( streq(kind, "enum") )
    made = t.create_enum();
  else
    made = t.create_udt(streq(kind, "union"));
  if ( !made )
    return fail("cannot create a %s", kind);
  undo_point();
  tinfo_code_t rc = t.set_named_type(nullptr, name, NTF_TYPE);
  if ( rc != TERR_OK )
    return fail("cannot create %s: %s", name, code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

// Finds a reference to one type inside another type's definition. Nested
// references are not followed: only direct dependents count.
struct ref_finder_t : public tinfo_visitor_t
{
  const char *target;
  uint32 target_ord;
  uint32 self_ord;
  bool found = false;
  ref_finder_t(const char *t, uint32 to, uint32 so) : tinfo_visitor_t(TVST_DEF), target(t), target_ord(to), self_ord(so) {}
  int idaapi visit_type(type_mods_t *, const tinfo_t &tif, const char *, const char *) override
  {
    if ( !tif.is_typeref() )
      return 0;
    uint32 ord = tif.get_ordinal();
    if ( ord != 0 && ord == self_ord )
      return 0; // the root: look inside its definition
    qstring n;
    if ( (ord != 0 && ord == target_ord) || (tif.get_type_name(&n) && n == target) )
    {
      found = true;
      return 1;
    }
    prune_now();
    return 0;
  }
};

// Deletes a local type. Types that refer to it block the deletion unless
// `force`; either way the dependents come back.
int izs_type_delete(const char *name, bool force)
{
  REQUIRE_DB();
  GUARD_BEGIN
  uint32 ordinal = get_type_ordinal(nullptr, name);
  if ( ordinal == 0 )
    return fail("no local type named %s", name);
  Json deps;
  deps.arr();
  size_t ndeps = 0;
  uint32 limit = get_ordinal_limit(nullptr);
  for ( uint32 ord = 1; ord < limit; ord++ )
  {
    const char *other = get_numbered_type_name(nullptr, ord);
    if ( other == nullptr || ord == ordinal )
      continue;
    tinfo_t t;
    if ( !t.get_numbered_type(nullptr, ord, BTF_TYPEDEF, false) )
      continue;
    ref_finder_t finder(name, ordinal, ord);
    finder.apply_to(t);
    if ( finder.found )
    {
      deps.str(other);
      ndeps++;
    }
  }
  deps.end_arr();
  bool deleted = false;
  if ( ndeps == 0 || force )
  {
    undo_point();
    deleted = del_numbered_type(nullptr, ordinal);
    if ( !deleted )
      return fail("cannot delete %s", name);
    types_changed();
  }
  Json j;
  j.obj().kvb("deleted", deleted);
  j.key("dependents").raw(deps.s);
  j.end_obj();
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
// Structures and unions

static void emit_udm(Json &j, const udm_t &m, size_t index)
{
  qstring type;
  print_type_text(&type, m.type);
  bool bf = m.is_bitfield();
  j.obj()
    .kv("index", (uint64_t)index)
    .kv("offset", m.offset / 8)
    .kv("size", bf ? 0 : m.size / 8)
    .key("bits");
  if ( bf )
    j.u64(m.size);
  else
    j.null();
  j.key("bit_offset");
  if ( bf )
    j.u64(m.offset % 8);
  else
    j.null();
  j.kv("name", qs(m.name))
    .kv("type", qs(type))
    .kv("cmt", qs(m.cmt))
    .kvb("gap", m.is_gap())
    .end_obj();
}

static int load_udt(const char *name, tinfo_t *t, udt_type_data_t *u)
{
  if ( !load_final(name, t) )
    return fail("no local type named %s", name);
  if ( !t->is_udt() )
    return fail("%s is not a structure or union", name);
  if ( !t->get_udt_details(u) )
    return fail("cannot read the members of %s", name);
  return 0;
}

int izs_udt_info(const char *name)
{
  REQUIRE_DB();
  GUARD_BEGIN
  tinfo_t t;
  udt_type_data_t u;
  int rc = load_udt(name, &t, &u);
  if ( rc != 0 )
    return rc;
  qstring real;
  if ( !t.get_type_name(&real) )
    real = name;
  Json j;
  j.obj()
    .kv("name", qs(real))
    .kv("ordinal", (uint64_t)get_type_ordinal(nullptr, real.c_str()))
    .kvb("is_union", u.is_union)
    .kv("size", (uint64_t)u.total_size)
    .key("members").arr();
  for ( size_t i = 0; i < u.size(); i++ )
    emit_udm(j, u[i], i);
  j.end_arr().end_obj();
  return ok(j);
  GUARD_END
}

#define LOAD_UDT(name) \
  tinfo_t t; udt_type_data_t u; \
  { int lrc = load_udt(name, &t, &u); if ( lrc != 0 ) return lrc; }
#define CHECK_INDEX(idx, count) \
  if ( (idx) >= (count) ) return fail("no member number %u", (unsigned)(idx))

// `offset` BADADDR appends at the end of a structure.
int izs_udt_add(const char *name, const char *member, const char *decl, uint64_t offset)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_UDT(name)
  tinfo_t ty;
  if ( !parse_type_text(decl, &ty) )
    return fail("cannot parse the type '%s'", decl);
  if ( offset == BADADDR )
    offset = u.is_union ? 0 : u.total_size;
  undo_point();
  tinfo_code_t rc = t.add_udm(member, ty, offset * 8, 0);
  if ( rc != TERR_OK )
    return fail("cannot add %s: %s", member, code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_udt_rename(const char *name, uint32 index, const char *newname)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_UDT(name)
  CHECK_INDEX(index, u.size());
  undo_point();
  tinfo_code_t rc = t.rename_udm(index, newname);
  if ( rc != TERR_OK )
    return fail("cannot rename the member: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_udt_retype(const char *name, uint32 index, const char *decl, bool force)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_UDT(name)
  CHECK_INDEX(index, u.size());
  tinfo_t ty;
  if ( !parse_type_text(decl, &ty) )
    return fail("cannot parse the type '%s'", decl);
  undo_point();
  tinfo_code_t rc = t.set_udm_type(index, ty, force ? ETF_MAY_DESTROY : 0);
  if ( rc != TERR_OK )
    return fail("cannot change the type: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_udt_delete(const char *name, uint32 index)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_UDT(name)
  CHECK_INDEX(index, u.size());
  undo_point();
  tinfo_code_t rc = t.del_udm(index);
  if ( rc != TERR_OK )
    return fail("cannot delete the member: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_udt_comment(const char *name, uint32 index, const char *text)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_UDT(name)
  CHECK_INDEX(index, u.size());
  undo_point();
  tinfo_code_t rc = t.set_udm_cmt(index, text, true);
  if ( rc != TERR_OK )
    return fail("cannot set the comment: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

// Cross references to a member: instructions with a struct-offset operand.
int izs_udt_xrefs(const char *name, uint32 index)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_UDT(name)
  CHECK_INDEX(index, u.size());
  tid_t tid = t.get_udm_tid(index);
  Json j;
  j.arr();
  if ( tid != BADADDR )
  {
    xrefblk_t xb;
    for ( bool more = xb.first_to(tid, XREF_ALL); more; more = xb.next_to() )
      emit_xref(j, xb.from, tid, xb.type, xb.from);
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
// Enumerations

static int load_enum(const char *name, tinfo_t *t, enum_type_data_t *e)
{
  if ( !load_final(name, t) )
    return fail("no local type named %s", name);
  if ( !t->is_enum() )
    return fail("%s is not an enumeration", name);
  if ( !t->get_enum_details(e) )
    return fail("cannot read the members of %s", name);
  return 0;
}

#define LOAD_ENUM(name) \
  tinfo_t t; enum_type_data_t e; \
  { int lrc = load_enum(name, &t, &e); if ( lrc != 0 ) return lrc; }

int izs_enum_info(const char *name)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_ENUM(name)
  qstring real;
  if ( !t.get_type_name(&real) )
    real = name;
  Json j;
  j.obj()
    .kv("name", qs(real))
    .kv("ordinal", (uint64_t)get_type_ordinal(nullptr, real.c_str()))
    .kvb("bitfield", t.is_bitmask_enum())
    .kv("width", (uint64_t)t.get_size())
    .kv("radix", (uint64_t)e.get_enum_radix())
    .key("members").arr();
  for ( size_t i = 0; i < e.size(); i++ )
  {
    j.obj()
      .kv("index", (uint64_t)i)
      .kv("name", qs(e[i].name))
      .kv("value", (uint64_t)e[i].value)
      .kv("cmt", qs(e[i].cmt))
      .end_obj();
  }
  j.end_arr().end_obj();
  return ok(j);
  GUARD_END
}

int izs_enum_add(const char *name, const char *member, uint64_t value)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_ENUM(name)
  undo_point();
  tinfo_code_t rc = t.add_edm(member, value);
  if ( rc != TERR_OK )
    return fail("cannot add %s: %s", member, code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_enum_rename(const char *name, uint32 index, const char *newname)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_ENUM(name)
  CHECK_INDEX(index, e.size());
  undo_point();
  tinfo_code_t rc = t.rename_edm(index, newname);
  if ( rc != TERR_OK )
    return fail("cannot rename the member: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_enum_set_value(const char *name, uint32 index, uint64_t value)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_ENUM(name)
  CHECK_INDEX(index, e.size());
  undo_point();
  tinfo_code_t rc = t.edit_edm(index, value);
  if ( rc != TERR_OK )
    return fail("cannot change the value: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_enum_delete(const char *name, uint32 index)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_ENUM(name)
  CHECK_INDEX(index, e.size());
  undo_point();
  tinfo_code_t rc = t.del_edm(index);
  if ( rc != TERR_OK )
    return fail("cannot delete the member: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_enum_comment(const char *name, uint32 index, const char *text)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_ENUM(name)
  CHECK_INDEX(index, e.size());
  undo_point();
  tinfo_code_t rc = t.set_edm_cmt(index, text);
  if ( rc != TERR_OK )
    return fail("cannot set the comment: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

int izs_enum_set_bitfield(const char *name, bool on)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_ENUM(name)
  undo_point();
  tinfo_code_t rc = t.set_enum_is_bitmask(on ? tinfo_t::ENUMBM_ON : tinfo_t::ENUMBM_OFF);
  if ( rc != TERR_OK )
    return fail("cannot change the flag mode: %s", code_str(rc));
  types_changed();
  return ok_null();
  GUARD_END
}

//--------------------------------------------------------------------------
// Stack frames

struct frame_t
{
  func_t *pfn = nullptr;
  tinfo_t tif;
  udt_type_data_t udt;
  range_t lvars, savregs, retaddr, args;
};

static int load_frame(ea_t ea, frame_t *f)
{
  f->pfn = get_func(ea);
  if ( f->pfn == nullptr )
    return fail("no function at %a", ea);
  if ( !get_func_frame(&f->tif, f->pfn) )
    return fail("the function has no stack frame");
  if ( !f->tif.get_udt_details(&f->udt) )
    return fail("cannot read the stack frame");
  get_frame_part(&f->lvars, f->pfn, FPC_LVARS);
  get_frame_part(&f->savregs, f->pfn, FPC_SAVREGS);
  get_frame_part(&f->retaddr, f->pfn, FPC_RETADDR);
  get_frame_part(&f->args, f->pfn, FPC_ARGS);
  return 0;
}

static void frame_changed(func_t *pfn)
{
  if ( g_hexrays )
    mark_cfunc_dirty(pfn->start_ea);
}

static const char *frame_kind(const frame_t &f, const udm_t &m)
{
  uint64 off = m.offset / 8;
  if ( m.is_retaddr() || (off >= f.retaddr.start_ea && off < f.retaddr.end_ea) )
    return "return_address";
  if ( m.is_savregs() || (off >= f.savregs.start_ea && off < f.savregs.end_ea) )
    return "saved_regs";
  return off < f.lvars.end_ea ? "local" : "arg";
}

static void emit_frame_part(Json &j, const char *key, const range_t &r)
{
  j.key(key).obj().kv("start", (uint64_t)r.start_ea).kv("end", (uint64_t)r.end_ea).end_obj();
}

int izs_frame_info(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  frame_t f;
  int rc = load_frame(ea, &f);
  if ( rc != 0 )
    return rc;
  Json j;
  j.obj()
    .kv("func", (uint64_t)f.pfn->start_ea)
    .kv("name", short_name(f.pfn->start_ea))
    .kv("size", (uint64_t)f.tif.get_size())
    .kv("locals_size", (uint64_t)f.pfn->frsize)
    .kv("saved_regs_size", (uint64_t)f.pfn->frregs)
    .kv("ret_size", (uint64_t)get_frame_retsize(f.pfn))
    .kv("args_size", (uint64_t)f.args.size())
    .kvi("fpd", (int64_t)f.pfn->fpd);
  emit_frame_part(j, "lvars", f.lvars);
  emit_frame_part(j, "saved_regs", f.savregs);
  emit_frame_part(j, "return_address", f.retaddr);
  emit_frame_part(j, "args", f.args);
  j.key("members").arr();
  for ( size_t i = 0; i < f.udt.size(); i++ )
  {
    const udm_t &m = f.udt[i];
    qstring type;
    print_type_text(&type, m.type);
    uint64_t soff = m.offset / 8;
    j.obj()
      .kv("index", (uint64_t)i)
      .kv("offset", soff)
      .kvi("fp_off", soff_to_fpoff(f.pfn, (uval_t)soff))
      .kv("size", m.size / 8)
      .kv("name", qs(m.name))
      .kv("type", qs(type))
      .kv("cmt", qs(m.cmt))
      .kv("kind", frame_kind(f, m))
      .end_obj();
  }
  j.end_arr().end_obj();
  return ok(j);
  GUARD_END
}

static int frame_index_at(const frame_t &f, uint64_t offset)
{
  for ( size_t i = 0; i < f.udt.size(); i++ )
    if ( f.udt[i].offset / 8 == offset )
      return (int)i;
  return -1;
}

#define LOAD_FRAME(ea) \
  frame_t f; \
  { int lrc = load_frame(ea, &f); if ( lrc != 0 ) return lrc; }

int izs_frame_define(ea_t ea, uint64_t offset, const char *name, const char *decl)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_FRAME(ea)
  tinfo_t ty;
  if ( !parse_type_text(decl, &ty) )
    return fail("cannot parse the type '%s'", decl);
  undo_point();
  if ( !add_frame_member(f.pfn, name[0] != '\0' ? name : nullptr, (uval_t)offset, ty) )
    return fail("cannot define a variable at frame offset %llX", (unsigned long long)offset);
  frame_changed(f.pfn);
  return ok_null();
  GUARD_END
}

int izs_frame_rename(ea_t ea, uint64_t offset, const char *name)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_FRAME(ea)
  int idx = frame_index_at(f, offset);
  if ( idx < 0 )
    return fail("no variable at frame offset %llX", (unsigned long long)offset);
  undo_point();
  tinfo_code_t rc = f.tif.rename_udm(idx, name);
  if ( rc != TERR_OK )
    return fail("cannot rename the variable: %s", code_str(rc));
  frame_changed(f.pfn);
  return ok_null();
  GUARD_END
}

int izs_frame_retype(ea_t ea, uint64_t offset, const char *decl, bool force)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_FRAME(ea)
  tinfo_t ty;
  if ( !parse_type_text(decl, &ty) )
    return fail("cannot parse the type '%s'", decl);
  undo_point();
  if ( !set_frame_member_type(f.pfn, (uval_t)offset, ty, nullptr, force ? ETF_MAY_DESTROY : 0) )
    return fail("cannot change the type at frame offset %llX", (unsigned long long)offset);
  frame_changed(f.pfn);
  return ok_null();
  GUARD_END
}

int izs_frame_delete(ea_t ea, uint64_t offset)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_FRAME(ea)
  int idx = frame_index_at(f, offset);
  if ( idx < 0 )
    return fail("no variable at frame offset %llX", (unsigned long long)offset);
  uint64_t end = offset + qmax((uint64_t)1, f.udt[idx].size / 8);
  undo_point();
  if ( !delete_frame_members(f.pfn, (uval_t)offset, (uval_t)end) )
    return fail("cannot delete the variable at frame offset %llX", (unsigned long long)offset);
  frame_changed(f.pfn);
  return ok_null();
  GUARD_END
}

int izs_frame_comment(ea_t ea, uint64_t offset, const char *text)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_FRAME(ea)
  int idx = frame_index_at(f, offset);
  if ( idx < 0 )
    return fail("no variable at frame offset %llX", (unsigned long long)offset);
  undo_point();
  tinfo_code_t rc = f.tif.set_udm_cmt(idx, text, true);
  if ( rc != TERR_OK )
    return fail("cannot set the comment: %s", code_str(rc));
  frame_changed(f.pfn);
  return ok_null();
  GUARD_END
}

// Instructions that use the variable at frame offset `offset`.
int izs_frame_xrefs(ea_t ea, uint64_t offset)
{
  REQUIRE_DB();
  GUARD_BEGIN
  LOAD_FRAME(ea)
  int idx = frame_index_at(f, offset);
  if ( idx < 0 )
    return fail("no variable at frame offset %llX", (unsigned long long)offset);
  uint64_t end = offset + qmax((uint64_t)1, f.udt[idx].size / 8);
  xreflist_t list;
  build_stkvar_xrefs(&list, f.pfn, (uval_t)offset, (uval_t)end);
  Json j;
  j.arr();
  for ( const xreflist_entry_t &x : list )
    emit_xref(j, x.ea, f.pfn->start_ea, x.type, x.ea);
  j.end_arr();
  return ok(j);
  GUARD_END
}


//--------------------------------------------------------------------------
// Function summary for the inspector panel

static const char *cc_name(cm_t cc)
{
  switch ( get_cc(cc) )
  {
    case CM_CC_CDECL:    return "__cdecl";
    case CM_CC_ELLIPSIS: return "__cdecl (variadic)";
    case CM_CC_STDCALL:  return "__stdcall";
    case CM_CC_PASCAL:   return "__pascal";
    case CM_CC_FASTCALL: return "__fastcall";
    case CM_CC_THISCALL: return "__thiscall";
    case CM_CC_SWIFT:    return "__swiftcall";
    case CM_CC_GOLANG:   return "golang";
    case CM_CC_SPECIAL:
    case CM_CC_SPECIALE: return "__usercall";
    case CM_CC_SPECIALP: return "__userpurge";
    case CM_CC_VOIDARG:  return "__cdecl";
    default:             return "unknown";
  }
}

int izs_func_summary(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  func_t *pfn = get_func(ea);
  if ( pfn == nullptr )
    return ok_null();
  Json j;
  j.obj()
    .kv("start", (uint64_t)pfn->start_ea)
    .kv("end", (uint64_t)pfn->end_ea)
    .kv("name", short_name(pfn->start_ea));
  segment_t *seg = getseg(pfn->start_ea);
  if ( seg != nullptr )
  {
    qstring name, cls;
    get_visible_segm_name(&name, seg);
    get_segm_class(&cls, seg);
    char perm[4] = {
      (seg->perm & SEGPERM_READ) ? 'R' : '-',
      (seg->perm & SEGPERM_WRITE) ? 'W' : '-',
      (seg->perm & SEGPERM_EXEC) ? 'X' : '-',
      '\0',
    };
    j.kv("segment", qs(name)).kv("seg_class", qs(cls)).kv("seg_perm", perm);
  }
  else
    j.kv("segment", "").kv("seg_class", "").kv("seg_perm", "");

  tinfo_t tif;
  const char *cc = "unknown";
  if ( get_tinfo(&tif, pfn->start_ea) && tif.is_func() )
    cc = cc_name(tif.get_cc());
  j.kv("convention", cc);
  // Print with the demangled name, minus its argument list: print_type would
  // use the sanitised name (FIND__SearchFiles).
  qstring proto;
  {
    std::string nm = short_name(pfn->start_ea);
    if ( !nm.empty() && nm.back() == ')' )
    {
      int depth = 0;
      for ( size_t i = nm.size(); i-- > 0; )
      {
        if ( nm[i] == ')' )
          depth++;
        else if ( nm[i] == '(' && --depth == 0 )
        {
          if ( i > 0 )
            nm.resize(i);
          break;
        }
      }
    }
    tinfo_t ft;
    bool have = get_tinfo(&ft, pfn->start_ea) && ft.is_func();
    if ( !have )
      have = guess_tinfo(&ft, pfn->start_ea) != GUESS_FUNC_FAILED && ft.is_func();
    if ( have )
      ft.print(&proto, nm.c_str(), PRTYPE_1LINE | PRTYPE_SEMI);
  }
  j.kv("prototype", qs(proto));

  qflow_chart_t fc("", pfn, BADADDR, BADADDR, FC_NOEXT);
  uint64_t edges = 0;
  for ( int i = 0; i < fc.size(); i++ )
    edges += fc.nsucc(i);
  j.kv("blocks", (uint64_t)fc.size()).kv("edges", edges);

  uint64_t locals = 0;
  uint64_t frame_size = 0;
  {
    frame_t f;
    if ( load_frame(pfn->start_ea, &f) == 0 )
    {
      frame_size = f.tif.get_size();
      for ( size_t i = 0; i < f.udt.size(); i++ )
        if ( strcmp(frame_kind(f, f.udt[i]), "local") == 0 && !f.udt[i].name.empty() )
          locals++;
    }
  }
  j.kv("locals", locals).kv("frame_size", frame_size);

  j.key("flags").arr();
  if ( (pfn->flags & FUNC_LIB) != 0 )
    j.str("lib");
  if ( (pfn->flags & FUNC_THUNK) != 0 )
    j.str("thunk");
  if ( (pfn->flags & FUNC_NORET) != 0 )
    j.str("noreturn");
  if ( (pfn->flags & FUNC_STATICDEF) != 0 )
    j.str("static");
  j.end_arr();

  // Distinct call targets, in order of first call.
  j.key("callees").arr();
  eavec_t seen;
  func_item_iterator_t fii(pfn);
  for ( bool more = fii.set(pfn); more && seen.size() < 200; more = fii.next_code() )
  {
    ea_t head = fii.current();
    insn_t insn;
    if ( decode_insn(&insn, head) <= 0 || !is_call_insn(insn) )
      continue;
    xrefblk_t xb;
    for ( bool m = xb.first_from(head, XREF_ALL); m; m = xb.next_from() )
    {
      if ( xb.to >= pfn->start_ea && xb.to < pfn->end_ea )
        continue;
      func_t *target = get_func(xb.to);
      ea_t at = target != nullptr ? target->start_ea : xb.to;
      if ( seen.has(at) )
        continue;
      seen.push_back(at);
      j.obj()
        .kv("ea", (uint64_t)at)
        .kv("name", short_name(at))
        .kvb("imported", target == nullptr || (target->flags & FUNC_THUNK) != 0)
        .end_obj();
    }
  }
  j.end_arr();
  j.end_obj();
  return ok(j);
  GUARD_END
}

} // extern "C"
