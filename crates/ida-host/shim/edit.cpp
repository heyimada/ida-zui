// Editing commands that belong to IDA's Edit menu: operand representation,
// arrays, typed strings, function properties, patching and marked positions.
// Same conventions as shim.cpp: REQUIRE_DB / GUARD_BEGIN, undo_point() before
// any mutation, JSON through Json.

#include <pro.h>
#include <ida.hpp>
#include <idp.hpp>
#include <bytes.hpp>
#include <frame.hpp>
#include <funcs.hpp>
#include <kernwin.hpp>
#include <moves.hpp>
#include <name.hpp>
#include <nalt.hpp>
#include <offset.hpp>
#include <segment.hpp>
#include <typeinf.hpp>
#include <ua.hpp>
#include <hexrays.hpp>

#include <algorithm>
#include <filesystem>
#include <string>
#include <vector>

#include "common.hpp"
#include "events.hpp"

std::string short_name(ea_t ea);

//--------------------------------------------------------------------------
// Operand representation

enum ReprKind
{
  R_HEX = 0, R_DEC, R_CHAR, R_BIN, R_OCT, R_NUM, R_OFFSET, R_OFFSET_SEG,
  R_STROFF, R_ENUM, R_STKVAR, R_NEGATE, R_INVERT, R_CLEAR,
};

// The operands of the item at `head` that can show a representation.
static std::vector<int> item_operands(ea_t head)
{
  std::vector<int> ops;
  flags64_t F = get_flags(head);
  if ( is_code(F) )
  {
    insn_t insn;
    if ( decode_insn(&insn, head) > 0 )
      for ( int i = 0; i < UA_MAXOP; i++ )
        if ( insn.ops[i].type != o_void )
          ops.push_back(i);
  }
  else
  {
    ops.push_back(0);
  }
  return ops;
}

// Resolves a struct path given as "Type\nunion_member\n..." to type ids.
static bool resolve_stroff_path(const std::string &spec, std::vector<tid_t> *ids, qstring *err)
{
  size_t pos = 0;
  tinfo_t cur;
  bool first = true;
  while ( pos <= spec.size() )
  {
    size_t nl = spec.find('\n', pos);
    std::string name = spec.substr(pos, nl == std::string::npos ? std::string::npos : nl - pos);
    pos = nl == std::string::npos ? spec.size() + 1 : nl + 1;
    if ( name.empty() )
      continue;
    if ( first )
    {
      tid_t tid = get_named_type_tid(name.c_str());
      if ( tid == BADNODE || !cur.get_named_type(name.c_str()) || !cur.is_udt() )
      {
        err->sprnt("'%s' is not a struct or union", name.c_str());
        return false;
      }
      ids->push_back(tid);
      first = false;
      continue;
    }
    int idx = cur.find_udm(name.c_str());
    if ( idx < 0 )
    {
      err->sprnt("'%s' has no member '%s'", cur.dstr(), name.c_str());
      return false;
    }
    udm_t udm;
    udm.offset = 0;
    if ( cur.get_udm(&udm, idx) < 0 )
    {
      err->sprnt("cannot read member '%s'", name.c_str());
      return false;
    }
    tid_t mid = cur.get_udm_tid(idx);
    if ( mid == BADNODE )
    {
      err->sprnt("member '%s' has no id", name.c_str());
      return false;
    }
    ids->push_back(mid);
    cur = udm.type;
  }
  if ( ids->empty() )
  {
    err->sprnt("empty struct path");
    return false;
  }
  return true;
}

// The reference type that fits the operand's size, as IDA picks for O: a
// dword operand is a 32-bit offset even in a 64-bit database.
static uint32 offset_reftype(ea_t head, int n)
{
  flags64_t F = get_flags(head);
  insn_t decoded;
  if ( is_code(F) && n >= 0 && n < UA_MAXOP && decode_insn(&decoded, head) > 0 )
  {
    switch ( decoded.ops[n].dtype )
    {
      case dt_byte: return REF_OFF8;
      case dt_word: return REF_OFF16;
      case dt_dword: return REF_OFF32;
      case dt_qword: return REF_OFF64;
      default: break;
    }
  }
  else if ( is_data(F) )
  {
    if ( is_byte(F) ) return REF_OFF8;
    if ( is_word(F) ) return REF_OFF16;
    if ( is_dword(F) ) return REF_OFF32;
    if ( is_qword(F) ) return REF_OFF64;
  }
  return get_default_reftype(head);
}

extern "C" {

int izs_set_op_repr(ea_t ea, int op, int kind, const char *text, int64_t num)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( op >= UA_MAXOP )
    return fail("operand %d does not exist", op);
  const int n = op < 0 ? OPND_ALL : op;
  ea_t head = get_item_head(ea);
  if ( !is_mapped(head) )
    return fail("%a is not in the database", ea);
  // The operands to visit one by one for the representations that need an
  // operand number of their own.
  std::vector<int> each;
  if ( n == OPND_ALL )
    each = item_operands(head);
  else
    each.push_back(n);

  // Resolved before the undo point so that a bad request changes nothing.
  std::vector<tid_t> path;
  tid_t enum_tid = BADNODE;
  if ( kind == R_STROFF )
  {
    qstring err;
    if ( !resolve_stroff_path(text, &path, &err) )
      return fail("%s", err.c_str());
  }
  else if ( kind == R_ENUM )
  {
    tinfo_t t;
    enum_tid = get_named_type_tid(text);
    if ( enum_tid == BADNODE || !t.get_named_type(text) || !t.is_enum() )
      return fail("'%s' is not an enum", text);
  }

  undo_point();
  bool done = false;
  insn_t insn;
  insn.ea = head;
  if ( is_code(get_flags(head)) )
    decode_insn(&insn, head);
  switch ( kind )
  {
    case R_HEX: done = op_hex(head, n); break;
    case R_DEC: done = op_dec(head, n); break;
    case R_CHAR: done = op_chr(head, n); break;
    case R_BIN: done = op_bin(head, n); break;
    case R_OCT: done = op_oct(head, n); break;
    case R_NUM: done = op_num(head, n); break;
    case R_OFFSET:
      for ( int i : each )
        done |= op_offset(head, i, offset_reftype(head, i), BADADDR, (ea_t)num) != 0;
      break;
    case R_OFFSET_SEG:
    {
      segment_t *s = getseg(head);
      ea_t base = s != nullptr ? get_segm_base(s) : 0;
      for ( int i : each )
        done |= op_offset(head, i, offset_reftype(head, i), BADADDR, base) != 0;
      break;
    }
    case R_STROFF:
      for ( int i : each )
        done |= op_stroff(insn, i, path.data(), (int)path.size(), (adiff_t)num);
      break;
    case R_ENUM:
      for ( int i : each )
        done |= op_enum(head, i, enum_tid, 0);
      break;
    case R_STKVAR:
      for ( int i : each )
        done |= op_stkvar(head, i);
      break;
    case R_NEGATE:
    case R_INVERT:
    {
      done = true;
      for ( int i : each )
      {
        flags64_t F = get_flags(head);
        bool now = kind == R_NEGATE ? is_invsign(head, F, i) : is_bnot(head, F, i);
        if ( now == (num != 0) )
          continue;
        done &= kind == R_NEGATE ? toggle_sign(head, i) : toggle_bnot(head, i);
      }
      break;
    }
    case R_CLEAR:
      done = clr_op_type(head, n);
      for ( int i : each )
      {
        flags64_t F = get_flags(head);
        if ( is_invsign(head, F, i) )
          toggle_sign(head, i);
        if ( is_bnot(head, F, i) )
          toggle_bnot(head, i);
        if ( is_forced_operand(head, i) )
          set_forced_operand(head, i, "");
      }
      break;
    default:
      return fail("unknown representation %d", kind);
  }
  if ( !done )
    return fail("cannot change the operand representation at %a", head);
  if ( g_hexrays )
  {
    func_t *pfn = get_func(head);
    if ( pfn != nullptr )
      mark_cfunc_dirty(pfn->start_ea);
  }
  return ok_null();
  GUARD_END
}

int izs_get_op_repr(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  ea_t head = get_item_head(ea);
  flags64_t F = get_flags(head);
  Json j;
  j.arr();
  for ( int n : item_operands(head) )
  {
    const char *kind = "default";
    std::string detail;
    if ( is_defarg(F, n) )
    {
      if ( is_off(F, n) )
      {
        kind = "offset";
        ea_t base = get_offbase(head, n);
        if ( base != BADADDR )
        {
          char buf[32];
          qsnprintf(buf, sizeof(buf), "%llX", (unsigned long long)base);
          detail = buf;
        }
      }
      else if ( is_stroff(F, n) )
      {
        kind = "struct_offset";
        tid_t path[MAXSTRUCPATH];
        adiff_t delta = 0;
        int len = get_stroff_path(path, &delta, head, n);
        for ( int i = 0; i < len; i++ )
        {
          qstring nm;
          get_tid_name(&nm, path[i]);
          if ( i != 0 )
            detail += ".";
          detail += qs(nm);
        }
      }
      else if ( is_enum(F, n) )
      {
        kind = "enum";
        uchar serial = 0;
        qstring nm;
        tid_t tid = get_enum_id(&serial, head, n);
        if ( tid != BADNODE )
          get_tid_name(&nm, tid);
        detail = qs(nm);
      }
      else if ( is_stkvar(F, n) )
        kind = "stack_var";
      else if ( is_seg(F, n) )
        kind = "segment";
      else if ( is_char(F, n) )
        kind = "char";
      else if ( is_fltnum(F, n) )
        kind = "float";
      else if ( is_custfmt(F, n) )
        kind = "custom";
      else if ( is_flag_for_operand(F, FF_N_NUMH, n) )
        kind = "hex";
      else if ( is_flag_for_operand(F, FF_N_NUMD, n) )
        kind = "decimal";
      else if ( is_flag_for_operand(F, FF_N_NUMO, n) )
        kind = "octal";
      else if ( is_flag_for_operand(F, FF_N_NUMB, n) )
        kind = "binary";
    }
    j.obj()
      .kv("op", (uint64_t)n)
      .kv("kind", kind)
      .kvb("negated", is_invsign(head, F, n))
      .kvb("inverted", is_bnot(head, F, n))
      .kv("detail", detail)
      .end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
// Arrays and typed strings

enum ElemKind { E_BYTE = 0, E_WORD, E_DWORD, E_QWORD, E_OWORD, E_FLOAT, E_DOUBLE, E_STRUCT };

static const char *const elem_names[] = {
  "byte", "word", "dword", "qword", "oword", "float", "double", "struct",
};

static flags64_t elem_flag(int elem)
{
  switch ( elem )
  {
    case E_WORD: return word_flag();
    case E_DWORD: return dword_flag();
    case E_QWORD: return qword_flag();
    case E_OWORD: return oword_flag();
    case E_FLOAT: return float_flag();
    case E_DOUBLE: return double_flag();
    case E_STRUCT: return stru_flag();
    default: return byte_flag();
  }
}

static asize_t elem_size(int elem, tid_t tid)
{
  switch ( elem )
  {
    case E_WORD: return 2;
    case E_DWORD:
    case E_FLOAT: return 4;
    case E_QWORD:
    case E_DOUBLE: return 8;
    case E_OWORD: return 16;
    case E_STRUCT:
    {
      tinfo_t t;
      if ( tid != BADNODE && t.get_type_by_tid(tid) )
      {
        size_t sz = t.get_size();
        return sz == BADSIZE ? 0 : (asize_t)sz;
      }
      return 0;
    }
    default: return 1;
  }
}

static void write_elem(Json &j, int elem, tid_t tid)
{
  j.obj().kv("kind", elem_names[elem]);
  if ( elem == E_STRUCT )
  {
    qstring nm;
    get_tid_name(&nm, tid);
    j.kv("name", qs(nm));
  }
  j.end_obj();
}

int izs_array_defaults(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  ea_t head = get_item_head(ea);
  if ( !is_mapped(head) )
    return fail("%a is not in the database", ea);
  flags64_t F = get_flags(head);
  int elem = E_BYTE;
  tid_t tid = BADNODE;
  bool is_array = false;
  asize_t esz = 1;
  if ( is_data(F) && !is_strlit(F) )
  {
    if ( is_word(F) ) elem = E_WORD;
    else if ( is_dword(F) ) elem = E_DWORD;
    else if ( is_qword(F) ) elem = E_QWORD;
    else if ( is_oword(F) ) elem = E_OWORD;
    else if ( is_float(F) ) elem = E_FLOAT;
    else if ( is_double(F) ) elem = E_DOUBLE;
    else if ( is_struct(F) )
    {
      opinfo_t oi;
      if ( get_opinfo(&oi, head, 0, F) != nullptr )
      {
        elem = E_STRUCT;
        tid = oi.tid;
      }
    }
    esz = elem_size(elem, tid);
    if ( esz == 0 )
    {
      elem = E_BYTE;
      tid = BADNODE;
      esz = 1;
    }
    is_array = get_item_size(head) > esz;
  }
  ea_t end = calc_max_item_end(head, ITEM_END_FIXUP | ITEM_END_INITED | ITEM_END_NAME | ITEM_END_XREF);
  uint64_t room = end > head ? (end - head) / esz : 0;
  uint64_t count = is_array ? get_item_size(head) / esz : qmax(room, (uint64_t)1);
  Json j;
  j.obj().key("elem");
  write_elem(j, elem, tid);
  j.kv("elem_size", (uint64_t)esz)
    .kv("count", qmin(count, (uint64_t)UINT32_MAX))
    .kv("max_count", qmin(room, (uint64_t)UINT32_MAX))
    .kvb("is_array", is_array)
    .end_obj();
  return ok(j);
  GUARD_END
}

int izs_make_array(ea_t ea, int elem, const char *name, uint32 count)
{
  REQUIRE_DB();
  GUARD_BEGIN
  ea_t head = get_item_head(ea);
  if ( !is_mapped(head) )
    return fail("%a is not in the database", ea);
  if ( count == 0 )
    return fail("an array needs at least one element");
  tid_t tid = BADNODE;
  if ( elem == E_STRUCT )
  {
    tid = get_named_type_tid(name);
    if ( tid == BADNODE )
      return fail("unknown structure '%s'", name);
  }
  asize_t esz = elem_size(elem, tid);
  if ( esz == 0 )
    return fail("the element has no size");
  asize_t total = esz * count;
  undo_point();
  del_items(head, DELIT_SIMPLE, total);
  if ( !create_data(head, elem_flag(elem), total, tid) )
    return fail("cannot create an array of %u at %a", count, head);
  return ok_null();
  GUARD_END
}

int izs_make_string_typed(ea_t ea, int kind, uint32 len)
{
  REQUIRE_DB();
  GUARD_BEGIN
  static const int types[] = {
    STRTYPE_C,       STRTYPE_C_16,    STRTYPE_C_32,
    STRTYPE_PASCAL,  STRTYPE_PASCAL_16, STRTYPE_PASCAL_32,
    STRTYPE_LEN2,    STRTYPE_LEN2_16, STRTYPE_LEN2_32,
    STRTYPE_LEN4,    STRTYPE_LEN4_16, STRTYPE_LEN4_32,
  };
  if ( kind < 0 || kind >= (int)qnumber(types) )
    return fail("unknown string type %d", kind);
  undo_point();
  flags64_t F = get_flags(ea);
  if ( is_data(F) )
    del_items(ea, DELIT_SIMPLE, 1);
  if ( !create_strlit(ea, len, types[kind]) )
    return fail("cannot create a string of this type at %a", ea);
  return ok_null();
  GUARD_END
}

//--------------------------------------------------------------------------
// Function properties

struct FuncEditC
{
  uint64_t end;
  int8_t flags[7];
  int64_t locals;
  int64_t saved_regs;
  int64_t purged;
};

static const uint32 func_flag_bits[7] = {
  FUNC_NORET, FUNC_FAR, FUNC_LIB, FUNC_STATICDEF, FUNC_THUNK, FUNC_FRAME, FUNC_BOTTOMBP,
};

int izs_func_edit_info(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  func_t *pfn = get_func(ea);
  if ( pfn == nullptr )
    return fail("no function at %a", ea);
  Json j;
  j.obj()
    .kv("start", (uint64_t)pfn->start_ea)
    .kv("end", (uint64_t)pfn->end_ea)
    .kv("name", short_name(pfn->start_ea))
    .kvb("noret", (pfn->flags & FUNC_NORET) != 0)
    .kvb("far", (pfn->flags & FUNC_FAR) != 0)
    .kvb("library", (pfn->flags & FUNC_LIB) != 0)
    .kvb("static_def", (pfn->flags & FUNC_STATICDEF) != 0)
    .kvb("thunk", (pfn->flags & FUNC_THUNK) != 0)
    .kvb("bp_frame", (pfn->flags & FUNC_FRAME) != 0)
    .kvb("bp_bottom", (pfn->flags & FUNC_BOTTOMBP) != 0)
    .kv("locals", (uint64_t)pfn->frsize)
    .kv("saved_regs", (uint64_t)pfn->frregs)
    .kv("purged", (uint64_t)pfn->argsize)
    .end_obj();
  return ok(j);
  GUARD_END
}

int izs_edit_func(ea_t ea, const FuncEditC *e)
{
  REQUIRE_DB();
  GUARD_BEGIN
  func_t *pfn = get_func(ea);
  if ( pfn == nullptr )
    return fail("no function at %a", ea);
  if ( e->saved_regs > 0xFFFF )
    return fail("saved registers: %lld bytes is too many", (long long)e->saved_regs);
  const ea_t start = pfn->start_ea;
  undo_point();
  if ( e->end != BADADDR && e->end != pfn->end_ea )
  {
    if ( e->end <= start )
      return fail("the end must be above the start %a", start);
    if ( !set_func_end(start, (ea_t)e->end) )
      return fail("cannot move the end of the function to %a", (ea_t)e->end);
    pfn = get_func(start);
    if ( pfn == nullptr )
      return fail("the function disappeared");
  }
  if ( e->locals >= 0 || e->saved_regs >= 0 || e->purged >= 0 )
  {
    asize_t frsize = e->locals >= 0 ? (asize_t)e->locals : pfn->frsize;
    ushort frregs = e->saved_regs >= 0 ? (ushort)e->saved_regs : pfn->frregs;
    asize_t argsize = e->purged >= 0 ? (asize_t)e->purged : pfn->argsize;
    if ( !set_frame_size(pfn, frsize, frregs, argsize) )
      return fail("cannot set the frame sizes");
    pfn = get_func(start);
    if ( pfn == nullptr )
      return fail("the function disappeared");
  }
  uint32 flags = pfn->flags;
  for ( int i = 0; i < 7; i++ )
  {
    if ( e->flags[i] < 0 )
      continue;
    if ( e->flags[i] != 0 )
      flags |= func_flag_bits[i];
    else
      flags &= ~func_flag_bits[i];
    // IDA remembers that the user decided on far-ness.
    if ( i == 1 )
      flags |= FUNC_USERFAR;
  }
  if ( flags != pfn->flags )
  {
    pfn->flags = flags;
    if ( !update_func(pfn) )
      return fail("cannot update the function");
  }
  if ( g_hexrays )
    mark_cfunc_dirty(start);
  return ok_null();
  GUARD_END
}

//--------------------------------------------------------------------------
// Patching

static std::string hex_of(const std::vector<uchar> &b)
{
  static const char hex[] = "0123456789ABCDEF";
  std::string s;
  for ( uchar c : b )
  {
    s += hex[c >> 4];
    s += hex[c & 15];
  }
  return s;
}

int izs_patch_bytes(ea_t ea, const uchar *bytes, size_t len)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( len == 0 )
    return fail("nothing to patch");
  for ( size_t i = 0; i < len; i++ )
    if ( !is_mapped(ea + i) )
      return fail("%a is not in the database", ea + i);
  undo_point();
  patch_bytes(ea, bytes, len);
  Json j;
  j.u64(len);
  return ok(j);
  GUARD_END
}

int izs_assemble(ea_t ea, const char *text, bool pad)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( (PH.flag & PR_ASSEMBLE) == 0 )
    return fail("the %s processor module has no assembler", inf_get_procname().c_str());
  ea_t head = get_item_head(ea);
  segment_t *seg = getseg(head);
  if ( seg == nullptr || !is_mapped(head) )
    return fail("%a is not in the database", ea);
  const ea_t cs = seg->sel;
  const ea_t ip = head - get_segm_base(seg);
  const bool use32 = seg->bitness >= 1;
  uchar buf[MAXSTR];
  ssize_t n = PH.assemble(buf, head, cs, ip, use32, text);
  if ( n <= 0 )
    return fail("cannot assemble '%s'", text);
  std::vector<uchar> out(buf, buf + n);
  flags64_t F = get_flags(head);
  const bool was_code = is_code(F);
  asize_t old_len = was_code ? get_item_size(head) : 0;
  if ( pad && old_len > (asize_t)n )
  {
    uchar nop[MAXSTR];
    ssize_t nn = PH.assemble(nop, head + n, cs, ip + n, use32, "nop");
    while ( nn > 0 && out.size() + nn <= old_len )
      out.insert(out.end(), nop, nop + nn);
  }
  for ( size_t i = 0; i < out.size(); i++ )
    if ( !is_mapped(head + i) )
      return fail("the instruction does not fit before the end of the segment");
  undo_point();
  patch_bytes(head, out.data(), out.size());
  if ( was_code )
  {
    // The patched bytes decode as new instructions.
    del_items(head, DELIT_SIMPLE, out.size());
    for ( ea_t a = head; a < head + out.size(); )
    {
      int sz = create_insn(a);
      if ( sz <= 0 )
        break;
      a += sz;
    }
  }
  Json j;
  j.obj()
    .kv("hex", hex_of(out))
    .kv("len", (uint64_t)n)
    .kv("old_len", (uint64_t)old_len)
    .end_obj();
  return ok(j);
  GUARD_END
}

struct patch_t
{
  ea_t ea;
  qoff64_t fpos;
  uchar orig;
  uchar now;
};

static int idaapi collect_patch(ea_t ea, qoff64_t fpos, uint64 o, uint64 v, void *ud)
{
  ((std::vector<patch_t> *)ud)->push_back({ ea, fpos, (uchar)o, (uchar)v });
  return 0;
}

static std::vector<patch_t> patches_in(ea_t lo, ea_t hi)
{
  std::vector<patch_t> v;
  visit_patched_bytes(lo, hi, collect_patch, &v);
  return v;
}

int izs_revert_patches(ea_t ea, uint64_t len)
{
  REQUIRE_DB();
  GUARD_BEGIN
  ea_t hi = len > BADADDR - ea ? BADADDR : ea + (ea_t)len;
  std::vector<patch_t> v = patches_in(ea, hi);
  const bool was_code = is_code(get_flags(ea));
  undo_point();
  uint64_t reverted = 0;
  for ( const patch_t &p : v )
    if ( revert_byte(p.ea) )
      reverted++;
  if ( was_code && reverted != 0 && hi != BADADDR )
  {
    // The restored bytes are the instructions that were there before.
    del_items(ea, DELIT_SIMPLE, hi - ea);
    for ( ea_t a = ea; a < hi; )
    {
      int sz = create_insn(a);
      if ( sz <= 0 )
        break;
      a += sz;
    }
  }
  Json j;
  j.u64(reverted);
  return ok(j);
  GUARD_END
}

int izs_patch_list(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  for ( const patch_t &p : patches_in(inf_get_min_ea(), inf_get_max_ea()) )
  {
    j.obj().kv("ea", (uint64_t)p.ea).kv("original", (uint64_t)p.orig).kv("patched", (uint64_t)p.now);
    j.key("file_offset");
    if ( p.fpos < 0 )
      j.null();
    else
      j.u64((uint64_t)p.fpos);
    j.end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_apply_patches(const char *output)
{
  REQUIRE_DB();
  GUARD_BEGIN
  namespace fs = std::filesystem;
  char in_buf[QMAXPATH];
  if ( get_input_file_path(in_buf, sizeof(in_buf)) <= 0 )
    return fail("the database has no input file");
  fs::path in = fs::u8path(in_buf);
  fs::path out = fs::u8path(output);
  std::error_code ec;
  if ( !fs::exists(in, ec) )
    return fail("the input file %s does not exist", in_buf);
  if ( fs::exists(out, ec) && fs::equivalent(in, out, ec) )
    return fail("refusing to overwrite the input file");
  if ( fs::absolute(out, ec).lexically_normal() == fs::absolute(in, ec).lexically_normal() )
    return fail("refusing to overwrite the input file");
  // IDA's file API: the SDK poisons the C stdio calls the C++ streams use.
  FILE *src = qfopen(in_buf, "rb");
  if ( src == nullptr )
    return fail("cannot read %s", in_buf);
  FILE *dst = qfopen(output, "wb");
  if ( dst == nullptr )
  {
    qfclose(src);
    return fail("cannot write %s", output);
  }
  uint64_t size = 0;
  {
    std::vector<uchar> chunk(1 << 20);
    for ( ;; )
    {
      size_t got = qfread(src, chunk.data(), chunk.size());
      if ( got == 0 )
        break;
      if ( qfwrite(dst, chunk.data(), got) != got )
      {
        qfclose(src);
        qfclose(dst);
        return fail("writing %s failed", output);
      }
      size += got;
    }
  }
  qfclose(src);
  uint32_t written = 0, skipped = 0;
  for ( const patch_t &p : patches_in(inf_get_min_ea(), inf_get_max_ea()) )
  {
    if ( p.fpos < 0 || (uint64_t)p.fpos >= size )
    {
      skipped++;
      continue;
    }
    uchar b = p.now;
    if ( qfseek(dst, p.fpos, SEEK_SET) != 0 || qfwrite(dst, &b, 1) != 1 )
    {
      qfclose(dst);
      return fail("writing %s failed", output);
    }
    written++;
  }
  if ( qfclose(dst) != 0 )
    return fail("writing %s failed", output);
  Json j;
  j.obj().kv("output", std::string(output)).kv("written", (uint64_t)written)
    .kv("skipped", (uint64_t)skipped).end_obj();
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
// Marked positions: IDA's bookmarks of the disassembly view.

static lochist_entry_t mark_entry(ea_t ea)
{
  lochist_entry_t e;
  idaplace_t pl(ea, 0);
  e.set_place(pl);
  e.renderer_info().rtype = TCCRT_FLAT;
  return e;
}

static bool mark_at_slot(uint32 slot, ea_t *ea, qstring *desc)
{
  lochist_entry_t e = mark_entry(0);
  uint32 idx = slot;
  qstring d;
  if ( !bookmarks_t::get(&e, &d, &idx, nullptr) || e.place() == nullptr )
    return false;
  *ea = e.place()->toea();
  *desc = d;
  return true;
}

int izs_mark_set(ea_t ea, const char *desc)
{
  REQUIRE_DB();
  GUARD_BEGIN
  ea_t head = get_item_head(ea);
  lochist_entry_t e = mark_entry(head);
  uint32 slot = bookmarks_t::find_index(e, nullptr);
  if ( slot == BOOKMARKS_BAD_INDEX )
  {
    for ( slot = 0; slot < MAX_MARK_SLOT; slot++ )
    {
      ea_t other;
      qstring d;
      if ( !mark_at_slot(slot, &other, &d) )
        break;
    }
    if ( slot >= MAX_MARK_SLOT )
      return fail("all %d mark slots are in use", MAX_MARK_SLOT);
  }
  undo_point();
  uint32 used = bookmarks_t::mark(e, slot, nullptr, desc, nullptr);
  if ( used == BOOKMARKS_BAD_INDEX )
    return fail("cannot mark %a", head);
  ev_note_display(head);
  Json j;
  j.u64(used);
  return ok(j);
  GUARD_END
}

int izs_mark_list(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  // The kernel has no enumeration: probe the slots. The listing's own place
  // type reports how many marks there are, so stop once all are found.
  lochist_entry_t probe = mark_entry(0);
  uint32 total = bookmarks_t::size(probe, nullptr);
  uint32 found = 0;
  for ( uint32 slot = 0; slot < MAX_MARK_SLOT && found < total; slot++ )
  {
    ea_t ea;
    qstring desc;
    if ( !mark_at_slot(slot, &ea, &desc) )
      continue;
    found++;
    j.obj().kv("slot", (uint64_t)slot).kv("ea", (uint64_t)ea).kv("desc", qs(desc)).end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_mark_delete(uint32 slot)
{
  REQUIRE_DB();
  GUARD_BEGIN
  ea_t ea;
  qstring desc;
  if ( !mark_at_slot(slot, &ea, &desc) )
    return fail("mark %u does not exist", slot);
  lochist_entry_t e = mark_entry(ea);
  undo_point();
  if ( !bookmarks_t::erase(e, slot, nullptr) )
    return fail("cannot delete mark %u", slot);
  ev_note_display(ea);
  return ok_null();
  GUARD_END
}

} // extern "C"
