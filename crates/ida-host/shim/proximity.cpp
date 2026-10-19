// Call neighbourhood of a function for the proximity view: callers up to
// `up` levels, callees down to `down` levels, with limits so that a function
// everybody calls cannot flood the view.

#include <pro.h>
#include <ida.hpp>
#include <bytes.hpp>
#include <funcs.hpp>
#include <idp.hpp>
#include <name.hpp>
#include <segment.hpp>
#include <ua.hpp>
#include <xref.hpp>

#include <algorithm>
#include <map>
#include <string>
#include <vector>

#include "common.hpp"

namespace
{
  const size_t SCAN_LIMIT = 400; // distinct neighbours looked at per function

  struct nbrs_t
  {
    std::vector<ea_t> callers;
    std::vector<ea_t> callees;
  };

  struct node_t
  {
    ea_t ea;
    int depth;
    nbrs_t n;
  };

  bool has(const std::vector<ea_t> &v, ea_t ea)
  {
    return std::find(v.begin(), v.end(), ea) != v.end();
  }

  // What a callee address is: the function containing it, else the address.
  ea_t target_of(ea_t to)
  {
    func_t *f = get_func(to);
    return f != nullptr ? f->start_ea : to;
  }

  // Distinct call (and tail-jump) targets of the function at `start`, and the
  // functions that call or jump to it. Data references are added to the
  // callees when `data` is set.
  nbrs_t neighbours(ea_t start, bool data)
  {
    nbrs_t r;
    func_t *pfn = get_func(start);
    if ( pfn != nullptr )
    {
      func_item_iterator_t fii(pfn);
      for ( bool more = fii.set(pfn); more && r.callees.size() < SCAN_LIMIT; more = fii.next_code() )
      {
        ea_t head = fii.current();
        insn_t insn;
        bool call = decode_insn(&insn, head) > 0 && is_call_insn(insn);
        xrefblk_t xb;
        for ( bool m = xb.first_from(head, XREF_ALL); m; m = xb.next_from() )
        {
          if ( xb.type == fl_F || (xb.to >= pfn->start_ea && xb.to < pfn->end_ea) )
            continue;
          bool is_jump = xb.type == fl_JN || xb.type == fl_JF;
          bool is_data = xb.type != fl_CN && xb.type != fl_CF && !is_jump;
          if ( is_data && (!data || is_code(get_flags(xb.to))) )
            continue;
          if ( !is_data && !call && !is_jump )
            continue;
          ea_t at = target_of(xb.to);
          if ( at == start || has(r.callees, at) )
            continue;
          r.callees.push_back(at);
        }
      }
    }
    xrefblk_t xb;
    for ( bool m = xb.first_to(start, XREF_ALL); m && r.callers.size() < SCAN_LIMIT; m = xb.next_to() )
    {
      if ( xb.type != fl_CN && xb.type != fl_CF && xb.type != fl_JN && xb.type != fl_JF )
        continue;
      func_t *from = get_func(xb.from);
      if ( from == nullptr || from->start_ea == start || has(r.callers, from->start_ea) )
        continue;
      r.callers.push_back(from->start_ea);
    }
    return r;
  }

  const char *kind_of(ea_t ea)
  {
    func_t *f = get_func(ea);
    if ( f != nullptr )
      return "func";
    segment_t *s = getseg(ea);
    if ( s != nullptr && s->type == SEG_XTRN )
      return "import";
    qstring n = get_short_name(ea);
    if ( n.empty() || !is_code(get_flags(ea)) )
    {
      // An import slot (IAT entry) is data named __imp_*.
      if ( n.length() > 6 && strncmp(n.c_str(), "__imp_", 6) == 0 )
        return "import";
      return "data";
    }
    return "func";
  }
}

// Nodes within `up` caller levels and `down` callee levels of `ea`, at most
// `fanout` neighbours per node and side and `max_nodes` in all. A node
// reports how many callers and callees it has in total, so the view can offer
// to expand the ones that were cut off.
extern "C" int izs_proximity(uint64_t ea, uint32_t up, uint32_t down, uint32_t fanout, uint32_t max_nodes, bool data)
{
  REQUIRE_DB();
  GUARD_BEGIN
  func_t *pfn = get_func((ea_t)ea);
  ea_t centre = pfn != nullptr ? pfn->start_ea : (ea_t)ea;
  if ( pfn == nullptr && !is_mapped(centre) )
    return fail("the address is not mapped");
  std::vector<node_t> nodes;
  std::map<ea_t, size_t> index;
  bool truncated = false;
  auto add = [&](ea_t at, int depth) -> bool {
    if ( index.count(at) )
      return false;
    if ( nodes.size() >= max_nodes )
    {
      truncated = true;
      return false;
    }
    index[at] = nodes.size();
    nodes.push_back({ at, depth, neighbours(at, data) });
    return true;
  };
  add(centre, 0);
  // Breadth first in each direction; the centre is shared.
  for ( int dir = -1; dir <= 1; dir += 2 )
  {
    uint32_t limit = dir < 0 ? up : down;
    std::vector<size_t> frontier = { 0 };
    for ( uint32_t level = 1; level <= limit && !frontier.empty(); level++ )
    {
      std::vector<size_t> next;
      for ( size_t ni : frontier )
      {
        std::vector<ea_t> list = dir < 0 ? nodes[ni].n.callers : nodes[ni].n.callees;
        size_t shown = 0;
        for ( ea_t at : list )
        {
          if ( shown >= fanout )
            break;
          bool known = index.count(at) != 0;
          if ( !known && !add(at, dir * (int)level) )
            continue;
          shown++;
          if ( !known )
            next.push_back(index[at]);
        }
      }
      frontier = next;
    }
  }

  Json j;
  j.obj().kv("centre", (uint64_t)centre).kvb("truncated", truncated).key("nodes").arr();
  for ( const node_t &n : nodes )
  {
    func_t *f = get_func(n.ea);
    j.obj()
      .kv("ea", (uint64_t)n.ea)
      .kv("name", short_name(n.ea))
      .kv("kind", kind_of(n.ea))
      .kvi("depth", n.depth)
      .kv("callers", (uint64_t)n.n.callers.size())
      .kv("callees", (uint64_t)n.n.callees.size())
      .kvb("library", f != nullptr && (f->flags & FUNC_LIB) != 0)
      .kvb("thunk", f != nullptr && (f->flags & FUNC_THUNK) != 0)
      .end_obj();
  }
  j.end_arr().key("edges").arr();
  // Every call between two of the nodes, whichever way the node was found.
  for ( const node_t &n : nodes )
    for ( ea_t to : n.n.callees )
      if ( index.count(to) )
        j.obj().kv("from", (uint64_t)n.ea).kv("to", (uint64_t)to).end_obj();
  j.end_arr().end_obj();
  return ok(j);
  GUARD_END
}
