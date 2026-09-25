// runtime/unavailable_stubs.c — GENERATED scaffolding (batch 156).
//
// Every symbol here is UNDEFINED at link time when compiling
// REasyQuant's local backtest entry (strategies/code/jq_wufu_local.py): some code
// path in the module graph calls it and nothing defines it. Purpose: let the
// LOCAL binary LINK so the run itself reports which paths are actually reached
// (loud failure beats a silent stub) — each one aborts with its own name.
//
// NOTE on naming: the names below are the macOS LINKER symbols (a leading `_`
// is added to every C identifier), so the C function is the name WITHOUT that
// leading underscore. Names that are not valid C identifiers go through an
// `__asm__` label carrying the linker name verbatim.
//
// Replace entries with real implementations as the run hits them; delete the
// entry once the local path runs end to end.
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>

// WEAK on purpose: a runtime object is linked into EVERY program, and these
// names (`_add`, `_get`, `_dict`, `_init`, `_date`, …) collide with ordinary
// program-level definitions — as STRONG symbols that produced "1 duplicate
// symbols" for ~6 official tests. Weak lets the program's own definition win
// and the stub only fill a genuine gap.

// Platform data-source entry points (`jqdatasdk.auth`, rqdatac/tushare/akshare/
// pyarrow readers). Per the project scope the PLATFORM APIs are not implemented,
// so the correct runtime behaviour is "the source is unavailable" — CPython
// raises there and the caller falls back to the local cache. Returning 0 keeps
// that fallback alive; aborting it would stop the local backtest at the first
// platform call (measured: `_auth` aborted inside `_jqdata_init`).
extern int64_t zeta_raise(int64_t code);
static int64_t zt_unavailable_soft(const char* what);

// `bs.login()` — baostock is NOT in the registry, so the member call compiles to
// the BARE symbol `login`, which the linker satisfies with libc's `login(3)`
// (utmp!) — measured as `EXC_BAD_ACCESS at 0x8` inside `getutmpx` called from
// `_baostock_login`. A weak definition here shadows that: the platform source is
// unavailable in the local path, and the caller's try/except takes the fallback.
int64_t login(void) { return zt_unavailable_soft("baostock.login"); }
// `logout` needs the SAME strong definition: as a weak symbol the dynamic
// linker preferred libsystem_c's `logout(3)` — measured: drvX4 SIGSEGV in
// `_platform_strnlen` via libc `logout` from `_baostock_logout`.
int64_t logout(void) { return zt_unavailable_soft("baostock.logout"); }

static int64_t zt_unavailable_soft(const char* what) {
    // The dedup suppresses only the MESSAGE — the RAISE must happen on EVERY call.
    // Suppressing the raise too made the SECOND call of a platform function return
    // 0 instead of raising: `pq.read_table(...)` in a later function returned a
    // null table and the caller then dereferenced `t.schema` (SEGV) instead of
    // taking its own try/except fallback.
    static const char* warned[64];
    static int n = 0;
    int seen = 0;
    for (int i = 0; i < n; i++) {
        if (warned[i] == what) { seen = 1; break; }
    }
    if (!seen) {
        if (n < 64) warned[n++] = what;
        fprintf(stderr, "PY-A: platform source `%s` is not available — falling back\n", what);
        fflush(stderr);
    }
    // RAISE, exactly like CPython does when the provider is missing/unauthorised:
    // the project wraps every provider call in try/except and takes the LOCAL
    // fallback. Returning 0 instead made `jqdatasdk.auth` look like it SUCCEEDED
    // ("authenticated successfully") and the code walked into the platform branch
    // with 0 handles (next crash).
    zeta_raise(1);
    return 0;
}

// PY-A (batch 422): the runtime half of batch 419's ghost guard. 419 turned a
// `[dynamic]<ty>::member` binding into a raise, but EVERY raise in this build
// carries code 1 — a corpus `except Exception as e: log(f"...: {e}")` therefore
// prints the same `1` for a missing module (107 such lines in one measured run)
// as for a lost dynamic member, and "did this call site actually fire before the
// crash?" cannot be answered from a run at all (batches 421 and 422 both had to
// fall back to reading IR). This names the member once on stderr and keeps 419's
// contract intact: raise, do not abort, so an enclosing except still takes its
// fallback.
int64_t zt_dyn_member_missing(const char* what) {
    // Same pointer-identity dedup as zt_unavailable_soft: the message is once
    // per ghost, the RAISE happens on every call (see the note there).
    static const char* noted[64];
    static int n = 0;
    int seen = 0;
    for (int i = 0; i < n; i++) {
        if (noted[i] == what) { seen = 1; break; }
    }
    if (!seen) {
        if (n < 64) noted[n++] = what;
        fprintf(stderr, "PY-A: dynamic receiver has no member `%s` — raising\n", what);
        fflush(stderr);
    }
    zeta_raise(1);
    return 0;
}

static int64_t zt_unavailable(const char* what) {
    fprintf(stderr,
            "PY-A: `%s` is NOT implemented in this build — the local backtest "
            "reached an unimplemented path.\n",
            what);
    fflush(stderr);
    // Debug escape hatch (same convention as py_stub_abort): with
    // ZETA_LENIENT_STUBS=1 these become warnings and return 0, so a single run
    // can reveal EVERY unimplemented path the local backtest touches instead of
    // stopping at the first one. The returned 0 is wrong by construction —
    // never use this mode to judge strategy results.
    if (getenv("ZETA_LENIENT_STUBS") != NULL) {
        return 0;
    }
    abort();
}

int64_t __attribute__((weak)) Any__filter(void) { return zt_unavailable("_Any__filter"); }
int64_t __attribute__((weak)) Cerebro(void) { return zt_unavailable("_Cerebro"); }
int64_t __attribute__((weak)) DataFrame__abs(void) { return zt_unavailable("_DataFrame__abs"); }
int64_t __attribute__((weak)) DataFrame__median(void) { return zt_unavailable("_DataFrame__median"); }
int64_t __attribute__((weak)) PyDate__to_pydatetime(void) { return zt_unavailable("_PyDate__to_pydatetime"); }
int64_t __attribute__((weak)) PyDate__tz_convert(void) { return zt_unavailable("_PyDate__tz_convert"); }
int64_t __attribute__((weak)) zt_stub_dynamic_str__isna(void) __asm__("_[dynamic]str__isna");
int64_t __attribute__((weak)) zt_stub_dynamic_str__isna(void) { return zt_unavailable("_[dynamic]str__isna"); }
int64_t __attribute__((weak)) zt_stub_dynamic_str__median(void) __asm__("_[dynamic]str__median");
int64_t __attribute__((weak)) zt_stub_dynamic_str__median(void) { return zt_unavailable("_[dynamic]str__median"); }
int64_t __attribute__((weak)) zt_stub_dynamic_str__notna(void) __asm__("_[dynamic]str__notna");
int64_t __attribute__((weak)) zt_stub_dynamic_str__notna(void) { return zt_unavailable("_[dynamic]str__notna"); }
int64_t __attribute__((weak)) __closure(void) { return zt_unavailable("___closure"); }
int64_t __attribute__((weak)) _jq_bar_types(void) { return zt_unavailable("__jq_bar_types"); }
int64_t __attribute__((weak)) _to_ts(void) { return zt_unavailable("__to_ts"); }
int64_t __attribute__((weak)) add(void) { return zt_unavailable("_add"); }
int64_t __attribute__((weak)) adddata(void) { return zt_unavailable("_adddata"); }
int64_t __attribute__((weak)) all(void) { return zt_unavailable("_all"); }
int64_t __attribute__((weak)) all_instruments(void) { return zt_unavailable_soft("_all_instruments"); }
int64_t __attribute__((weak)) any(void) { return zt_unavailable("_any"); }
int64_t __attribute__((weak)) auth(void) { return zt_unavailable_soft("_auth"); }
int64_t __attribute__((weak)) backend_datasrc_adjustment__anchor_to_reference(void) { return zt_unavailable("_backend_datasrc_adjustment__anchor_to_reference"); }
int64_t __attribute__((weak)) backend_datasrc_adjustment__apply_qfq_adjustment(void) { return zt_unavailable("_backend_datasrc_adjustment__apply_qfq_adjustment"); }
int64_t __attribute__((weak)) backend_datasrc_calibration__DataCalibrator___reference_loader(void) { return zt_unavailable("_backend_datasrc_calibration__DataCalibrator___reference_loader"); }
int64_t __attribute__((weak)) backend_datasrc_fund_adj_adjustment___fetch_fund_adj(void) { return zt_unavailable("_backend_datasrc_fund_adj_adjustment___fetch_fund_adj"); }
int64_t __attribute__((weak)) backend_datasrc_fund_adj_adjustment__apply_fund_adj_qfq(void) { return zt_unavailable("_backend_datasrc_fund_adj_adjustment__apply_fund_adj_qfq"); }
int64_t __attribute__((weak)) backend_datasrc_market_data___baostock_login(void) { return zt_unavailable("_backend_datasrc_market_data___baostock_login"); }
int64_t __attribute__((weak)) backend_datasrc_market_data___baostock_logout(void) { return zt_unavailable("_backend_datasrc_market_data___baostock_logout"); }
int64_t __attribute__((weak)) backend_datasrc_market_data__fetch_stock_data(void) { return zt_unavailable("_backend_datasrc_market_data__fetch_stock_data"); }
int64_t __attribute__((weak)) backend_strategy_backtrader_backend__BacktraderBackend(void) { return zt_unavailable("_backend_strategy_backtrader_backend__BacktraderBackend"); }
int64_t __attribute__((weak)) backend_strategy_nautilus_backend__NautilusBackend___context_factory(void) { return zt_unavailable("_backend_strategy_nautilus_backend__NautilusBackend___context_factory"); }
int64_t __attribute__((weak)) backend_strategy_nautilus_backend__NautilusBackend___price_lookup(void) { return zt_unavailable("_backend_strategy_nautilus_backend__NautilusBackend___price_lookup"); }
// Nested `_Impl` of NautilusJqStrategy::_make (nautilus-only path): its
// `self._jq_bar_types()` / `self.subscribe_bars()` callsite lowers against the
// OUTER class (receiver typed `module__NautilusJqStrategy`) — no definition
// exists (external nautilus base / never-emitted nested class).
int64_t __attribute__((weak)) backend_strategy_nautilus_backend__NautilusJqStrategy___jq_bar_types(void) { return zt_unavailable("_backend_strategy_nautilus_backend__NautilusJqStrategy___jq_bar_types"); }
int64_t __attribute__((weak)) backend_strategy_nautilus_backend__NautilusJqStrategy__subscribe_bars(void) { return zt_unavailable("_backend_strategy_nautilus_backend__NautilusJqStrategy__subscribe_bars"); }
int64_t __attribute__((weak)) backend_strategy_wufu_backend__LocalBackend___price_lookup(void) { return zt_unavailable("_backend_strategy_wufu_backend__LocalBackend___price_lookup"); }
int64_t __attribute__((weak)) cache_clear(void) { return zt_unavailable("_cache_clear"); }
int64_t __attribute__((weak)) call(void) { return zt_unavailable("_call"); }
int64_t __attribute__((weak)) clear(void) { return zt_unavailable("_clear"); }
int64_t __attribute__((weak)) clip(void) { return zt_unavailable("_clip"); }
int64_t __attribute__((weak)) cls(void) { return zt_unavailable("_cls"); }
int64_t __attribute__((weak)) condition(void) { return zt_unavailable("_condition"); }
int64_t __attribute__((weak)) date(void) { return zt_unavailable("_date"); }
int64_t __attribute__((weak)) decimal__Decimal(void) { return zt_unavailable("_decimal__Decimal"); }
int64_t __attribute__((weak)) decode(void) { return zt_unavailable("_decode"); }
int64_t __attribute__((weak)) dict(void) { return zt_unavailable("_dict"); }
int64_t __attribute__((weak)) download(void) { return zt_unavailable_soft("_download"); }
int64_t __attribute__((weak)) encode(void) { return zt_unavailable("_encode"); }
int64_t __attribute__((weak)) from_int(void) { return zt_unavailable("_from_int"); }
int64_t __attribute__((weak)) from_str(void) { return zt_unavailable("_from_str"); }
int64_t __attribute__((weak)) fund_daily(void) { return zt_unavailable_soft("_fund_daily"); }
int64_t __attribute__((weak)) fund_etf_category_sina(void) { return zt_unavailable_soft("_fund_etf_category_sina"); }
int64_t __attribute__((weak)) fund_etf_hist_em(void) { return zt_unavailable_soft("_fund_etf_hist_em"); }
int64_t __attribute__((weak)) get(void) { return zt_unavailable("_get"); }
int64_t __attribute__((weak)) get_level_values(void) { return zt_unavailable("_get_level_values"); }
int64_t __attribute__((weak)) get_loc(void) { return zt_unavailable("_get_loc"); }
int64_t __attribute__((weak)) get_row_data(void) { return zt_unavailable_soft("_get_row_data"); }
int64_t __attribute__((weak)) getsignal(void) { return zt_unavailable_soft("_getsignal"); }
int64_t __attribute__((weak)) getvalue(void) { return zt_unavailable("_getvalue"); }
int64_t __attribute__((weak)) index_components(void) { return zt_unavailable_soft("_index_components"); }
int64_t __attribute__((weak)) index_daily(void) { return zt_unavailable_soft("_index_daily"); }
int64_t __attribute__((weak)) init(void) { return zt_unavailable_soft("_init"); }
int64_t __attribute__((weak)) intersection(void) { return zt_unavailable("_intersection"); }
int64_t __attribute__((weak)) isna(void) { return zt_unavailable("_isna"); }
int64_t __attribute__((weak)) limit(void) { return zt_unavailable("_limit"); }
int64_t __attribute__((weak)) main_thread(void) { return zt_unavailable("_main_thread"); }
int64_t __attribute__((weak)) max(void) { return zt_unavailable("_max"); }
int64_t __attribute__((weak)) min(void) { return zt_unavailable("_min"); }
int64_t __attribute__((weak)) nautilus_trader_model_data__Bar(void) { return zt_unavailable("_nautilus_trader_model_data__Bar"); }
int64_t __attribute__((weak)) nautilus_trader_model_identifiers__InstrumentId(void) { return zt_unavailable("_nautilus_trader_model_identifiers__InstrumentId"); }
int64_t __attribute__((weak)) nautilus_trader_model_identifiers__Symbol(void) { return zt_unavailable("_nautilus_trader_model_identifiers__Symbol"); }
int64_t __attribute__((weak)) nautilus_trader_model_identifiers__Venue(void) { return zt_unavailable("_nautilus_trader_model_identifiers__Venue"); }
int64_t __attribute__((weak)) nautilus_trader_model_instruments__Equity(void) { return zt_unavailable("_nautilus_trader_model_instruments__Equity"); }
int64_t __attribute__((weak)) nautilus_trader_model_objects__Price(void) { return zt_unavailable("_nautilus_trader_model_objects__Price"); }
int64_t __attribute__((weak)) nautilus_trader_model_objects__Quantity(void) { return zt_unavailable("_nautilus_trader_model_objects__Quantity"); }
int64_t __attribute__((weak)) normalize(void) { return zt_unavailable("_normalize"); }
int64_t __attribute__((weak)) pro_api(void) { return zt_unavailable_soft("_pro_api"); }
int64_t __attribute__((weak)) query_all_stock(void) { return zt_unavailable_soft("_query_all_stock"); }
int64_t __attribute__((weak)) query_history_k_data_plus(void) { return zt_unavailable_soft("_query_history_k_data_plus"); }
int64_t __attribute__((weak)) query_hs300_stocks(void) { return zt_unavailable_soft("_query_hs300_stocks"); }
int64_t __attribute__((weak)) query_zz500_stocks(void) { return zt_unavailable_soft("_query_zz500_stocks"); }
int64_t __attribute__((weak)) read_table(void) { return zt_unavailable_soft("_read_table"); }
// Arity-mangled variant: a BOUND method call passes the receiver, so
// `pq.read_table(path)` compiles to `read_table_2(recv, path)`. Without this
// entry the linker picked a zero-returning stand-in and the caller walked into
// `t.schema.metadata` with t == 0 (SEGV at load_metadata + 184 instead of the
// project's try/except taking the fallback).
int64_t __attribute__((weak)) read_table_2(int64_t a, int64_t b) {
    (void)a; (void)b;
    return zt_unavailable_soft("_read_table");
}
int64_t __attribute__((weak)) replace_schema_metadata(void) { return zt_unavailable_soft("_replace_schema_metadata"); }
int64_t __attribute__((weak)) set_slippage_perc(void) { return zt_unavailable("_set_slippage_perc"); }
int64_t __attribute__((weak)) setcash(void) { return zt_unavailable("_setcash"); }
int64_t __attribute__((weak)) setdefault(void) { return zt_unavailable("_setdefault"); }
int64_t __attribute__((weak)) stock_zh_a_hist(void) { return zt_unavailable_soft("_stock_zh_a_hist"); }
int64_t __attribute__((weak)) subscribe_bars(void) { return zt_unavailable("_subscribe_bars"); }
int64_t __attribute__((weak)) update(void) { return zt_unavailable("_update"); }
int64_t __attribute__((weak)) values(void) { return zt_unavailable("_values"); }
int64_t __attribute__((weak)) write_table(void) { return zt_unavailable_soft("_write_table"); }

// Added for the ZETA_NO_OPT=1 (unoptimized) link: these paths are
// optimized away in the normal build, so they never showed up as undefined.
int64_t __attribute__((weak)) PyDate__astimezone(void) { return zt_unavailable("_PyDate__astimezone"); }
int64_t __attribute__((weak)) add_data(void) { return zt_unavailable("_add_data"); }
int64_t __attribute__((weak)) add_instrument(void) { return zt_unavailable("_add_instrument"); }
int64_t __attribute__((weak)) add_strategy(void) { return zt_unavailable("_add_strategy"); }
int64_t __attribute__((weak)) add_venue(void) { return zt_unavailable("_add_venue"); }
int64_t __attribute__((weak)) addanalyzer(void) { return zt_unavailable("_addanalyzer"); }
int64_t __attribute__((weak)) addstrategy(void) { return zt_unavailable("_addstrategy"); }
int64_t __attribute__((weak)) equity(void) { return zt_unavailable("_equity"); }
int64_t __attribute__((weak)) host_str_replace_2(void) { return zt_unavailable("_host_str_replace_2"); }
int64_t __attribute__((weak)) isoformat(void) { return zt_unavailable("_isoformat"); }
int64_t __attribute__((weak)) make_price(void) { return zt_unavailable("_make_price"); }
int64_t __attribute__((weak)) make_qty(void) { return zt_unavailable("_make_qty"); }
int64_t __attribute__((weak)) map(void) { return zt_unavailable("_map"); }
int64_t __attribute__((weak)) market(void) { return zt_unavailable("_market"); }
int64_t __attribute__((weak)) py_dt_now_1(void) { return zt_unavailable("_py_dt_now_1"); }
int64_t __attribute__((weak)) py_file_open_3(void) { return zt_unavailable("_py_file_open_3"); }
int64_t __attribute__((weak)) py_logger_debug_9(void) { return zt_unavailable("_py_logger_debug_9"); }
int64_t __attribute__((weak)) routine(void) { return zt_unavailable("_routine"); }
int64_t __attribute__((weak)) sort_data(void) { return zt_unavailable("_sort_data"); }
int64_t __attribute__((weak)) submit_order(void) { return zt_unavailable("_submit_order"); }
int64_t __attribute__((weak)) timestamp(void) { return zt_unavailable("_timestamp"); }
int64_t __attribute__((weak)) to_pydatetime(void) { return zt_unavailable("_to_pydatetime"); }

// nautilus engine constructors — never reached with `--engine local`.
int64_t __attribute__((weak)) nautilus_trader_backtest_engine__BacktestEngine(void) { return zt_unavailable("_nautilus_trader_backtest_engine__BacktestEngine"); }
int64_t __attribute__((weak)) nautilus_trader_model_objects__Money(void) { return zt_unavailable("_nautilus_trader_model_objects__Money"); }

// `logger.info(...)` reached through a closure whose receiver type was lost
// (diagnostic-only link aid; the normal build type-checks it).
int64_t __attribute__((weak)) info(void) { return zt_unavailable("_info"); }

// batch 159 追加: still-unresolved data-layer / cross-module-name symbols.
int64_t __attribute__((weak)) filter(void) { return zt_unavailable("_filter"); }
int64_t zt_stub_pd_Timestamp__date(void) __asm__("_pd.Timestamp__date");
int64_t __attribute__((weak)) zt_stub_pd_Timestamp__date(void) { return zt_unavailable("_pd.Timestamp__date"); }
int64_t __attribute__((weak)) strategies_code_jq_shim__OrderCost(void) { return zt_unavailable("_strategies_code_jq_shim__OrderCost"); }
int64_t __attribute__((weak)) strategies_code_jq_shim__PriceRelatedSlippage(void) { return zt_unavailable("_strategies_code_jq_shim__PriceRelatedSlippage"); }
int64_t __attribute__((weak)) strategies_code_jq_shim__attribute_history(void) { return zt_unavailable("_strategies_code_jq_shim__attribute_history"); }
