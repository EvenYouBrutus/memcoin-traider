from __future__ import annotations

import base64
import csv
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone, timedelta
from pathlib import Path
from typing import Any

# ============================================================
# CONFIG
# ============================================================

# Completely public Solana RPC. No API key.
RPC_URL = os.getenv(
    "SOLANA_RPC_URL",
    "https://api.mainnet-beta.solana.com",
)

# Completely public GeckoTerminal API. No API key.
GECKO_BASE = (
    "https://api.geckoterminal.com/api/v2"
)

# IMPORTANT:
# Start with a small period first.
#
# Example:
#   2026-09-01 -> 2026-09-16
#
# Then expand after you confirm everything works.
START_DATE = "2026-09-01"
END_DATE = "2026-09-16"

# Maximum graduation events to collect.
MAX_GRADUATIONS = 1000

# Number of signatures requested from Solana per page.
SIGNATURE_PAGE_SIZE = 100

# Number of transactions fetched concurrently.
TX_WORKERS = 1

# GeckoTerminal public API is ~10 requests/minute.
# Keep this conservative.
GECKO_DELAY = 6.5

# How many minutes after graduation to download.
HORIZON_MINUTES = 30

# Backtest.
POSITION_SIZE_SOL = 0.10
INITIAL_CAPITAL_SOL = 10.0

TAKE_PROFIT = 0.80       # +80%
STOP_LOSS = 0.25         # -25%
MAX_HOLD_MINUTES = 20

# Conservative fee approximation.
# This is not an exact historical execution cost.
FEE_RATE = 0.005

# Public RPC can rate-limit.
RPC_DELAY = 0.10
RPC_RETRIES = 3

# GeckoTerminal returns OHLCV candles.
# We use 1-minute candles.
TIMEFRAME = "minute"
AGGREGATE = 1

DATA_DIR = Path("data")

GRADUATIONS_FILE = (
    DATA_DIR / "graduations.csv"
)

CANDLES_FILE = (
    DATA_DIR / "candles.csv"
)

BACKTEST_FILE = (
    DATA_DIR / "backtest.csv"
)

SUMMARY_FILE = (
    DATA_DIR / "summary.txt"
)

CACHE_DIR = DATA_DIR / "cache"

RPC_CACHE_DIR = CACHE_DIR / "rpc"

POOL_CACHE_DIR = CACHE_DIR / "pools"

OHLC_CACHE_DIR = CACHE_DIR / "ohlc"


# ============================================================
# SOLANA PROGRAM IDS
# ============================================================

PUMP_PROGRAM = (
    "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
)

PUMPSWAP_PROGRAM = (
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"
)

WSOL = (
    "So11111111111111111111111111111111111111112"
)


# ============================================================
# HELPERS
# ============================================================

def ensure_dirs() -> None:
    for path in (
        DATA_DIR,
        CACHE_DIR,
        RPC_CACHE_DIR,
        POOL_CACHE_DIR,
        OHLC_CACHE_DIR,
    ):
        path.mkdir(
            parents=True,
            exist_ok=True,
        )


def dt_from_date(
    value: str,
) -> datetime:

    dt = datetime.strptime(
        value,
        "%Y-%m-%d",
    )

    return dt.replace(
        tzinfo=timezone.utc
    )


def date_to_ts(
    value: str,
) -> int:

    return int(
        dt_from_date(value).timestamp()
    )


def ts_to_iso(
    ts: int,
) -> str:

    return (
        datetime.fromtimestamp(
            ts,
            tz=timezone.utc,
        )
        .isoformat()
        .replace("+00:00", "Z")
    )


def iso_to_ts(
    value: str,
) -> int:

    return int(
        datetime.fromisoformat(
            value.replace("Z", "+00:00")
        ).timestamp()
    )


def safe_float(
    value: Any,
) -> float | None:

    try:
        if value is None:
            return None

        return float(value)

    except (
        TypeError,
        ValueError,
    ):
        return None


def sha256_8(
    text: str,
) -> bytes:

    return hashlib.sha256(
        text.encode("utf-8")
    ).digest()[:8]


def anchor_discriminator(
    name: str,
) -> bytes:

    return sha256_8(
        f"global:{name}"
    )


# Pump.fun "migrate" Anchor discriminator.
MIGRATE_DISCRIMINATOR = (
    anchor_discriminator("migrate")
)


# ============================================================
# HELPER: Generic HTTP
# ============================================================

def http_json(
    url: str,
    *,
    method: str = "GET",
    payload: dict[str, Any] | None = None,
    headers: dict[str, str] | None = None,
    retries: int = 5,
) -> dict[str, Any]:

    body = None

    request_headers = {
        "User-Agent": "pumpfun-public-backtester/1.0",
        "Accept": "application/json",
    }

    if headers:
        request_headers.update(
            headers
        )

    if payload is not None:
        body = json.dumps(
            payload
        ).encode("utf-8")

        request_headers[
            "Content-Type"
        ] = "application/json"

    request = urllib.request.Request(
        url,
        data=body,
        method=method,
        headers=request_headers,
    )

    last_error: Exception | None = None

    for attempt in range(
        1,
        retries + 1,
    ):

        try:

            with urllib.request.urlopen(
                request,
                timeout=60,
            ) as response:

                raw = response.read()

            return json.loads(
                raw.decode("utf-8")
            )

        except urllib.error.HTTPError as exc:

            last_error = exc

            body_text = ""

            try:
                body_text = (
                    exc.read()
                    .decode(
                        "utf-8",
                        errors="replace",
                    )
                )
            except Exception:
                pass

            if exc.code in (
                408,
                429,
                500,
                502,
                503,
                504,
            ):

                sleep_for = min(
                    2 ** (attempt - 1),
                    30,
                )

                print(
                    f"HTTP {exc.code}; "
                    f"retry {attempt}/{retries} "
                    f"in {sleep_for}s"
                )

                if body_text:
                    print(
                        body_text[:300]
                    )

                time.sleep(
                    sleep_for
                )

                continue

            raise RuntimeError(
                f"HTTP {exc.code}: "
                f"{body_text[:1000]}"
            ) from exc

        except (
            urllib.error.URLError,
            TimeoutError,
            ConnectionError,
            json.JSONDecodeError,
        ) as exc:

            last_error = exc

            if attempt >= retries:
                break

            sleep_for = min(
                2 ** (attempt - 1),
                30,
            )

            print(
                f"Network error: {exc}; "
                f"retry {attempt}/{retries} "
                f"in {sleep_for}s"
            )

            time.sleep(
                sleep_for
            )

    raise RuntimeError(
        f"Request failed after {retries} attempts: "
        f"{last_error}"
    )


# ============================================================
# HELPER: Solana RPC (targeted, not bulk scan)
# ============================================================

_rpc_id = 0


def solana_rpc(
    method: str,
    params: list[Any],
) -> Any:

    global _rpc_id

    _rpc_id += 1

    payload = {
        "jsonrpc": "2.0",
        "id": _rpc_id,
        "method": method,
        "params": params,
    }

    data = http_json(
        RPC_URL,
        method="POST",
        payload=payload,
        retries=RPC_RETRIES,
    )

    if data.get("error") is not None:
        raise RuntimeError(
            f"RPC {method} error: "
            f"{data['error']}"
        )

    if "result" not in data:
        raise RuntimeError(
            f"RPC {method}: no result"
        )

    time.sleep(
        RPC_DELAY
    )

    return data["result"]


# ============================================================
# HELPER: Base58
# ============================================================

BASE58_ALPHABET = (
    "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
)

BASE58_MAP = {
    char: index
    for index, char in enumerate(
        BASE58_ALPHABET
    )
}


def base58_decode(
    value: str,
) -> bytes:

    number = 0

    for char in value:
        number = (
            number * 58
            + BASE58_MAP[char]
        )

    raw = number.to_bytes(
        max(
            1,
            (number.bit_length() + 7) // 8,
        ),
        "big",
    )

    leading = 0

    for char in value:
        if char == "1":
            leading += 1
        else:
            break

    return (
        b"\x00" * leading
        + raw
    )


# ============================================================
# PUMP.FUN MIGRATION DETECTION
# ============================================================

def instruction_is_migrate(
    instruction: dict[str, Any],
) -> bool:

    program_id = instruction.get(
        "programId"
    )

    if program_id != PUMP_PROGRAM:
        return False

    # jsonParsed instructions may not expose
    # raw data for every instruction.
    data = instruction.get(
        "data"
    )

    if not isinstance(data, str):
        return False

    try:
        decoded = base58_decode(
            data
        )
    except Exception:
        return False

    return decoded.startswith(
        MIGRATE_DISCRIMINATOR
    )


# ============================================================
# ACCOUNT EXTRACTION
# ============================================================

def normalize_account_key(
    value: Any,
) -> str | None:

    if isinstance(value, str):
        return value

    if isinstance(value, dict):
        pubkey = value.get(
            "pubkey"
        )

        if isinstance(
            pubkey,
            str,
        ):
            return pubkey

    return None


def account_keys(
    tx: dict[str, Any],
) -> list[str]:

    message = (
        tx.get("transaction", {})
        .get("message", {})
    )

    keys = message.get(
        "accountKeys",
        [],
    )

    result = []

    for key in keys:

        normalized = (
            normalize_account_key(
                key
            )
        )

        if normalized:
            result.append(
                normalized
            )

    return result


def get_mint_candidates(
    tx: dict[str, Any],
) -> list[str]:

    keys = account_keys(
        tx
    )

    result = []

    for key in keys:

        # Pump.fun mints normally use the
        # familiar "...pump" suffix.
        if key.endswith("pump"):
            result.append(
                key
            )

    # Remove duplicates preserving order.
    return list(
        dict.fromkeys(result)
    )


# ============================================================
# FETCH TRANSACTION (with caching)
# ============================================================

def tx_cache_path(
    signature: str,
) -> Path:

    digest = hashlib.sha256(
        signature.encode()
    ).hexdigest()

    return (
        RPC_CACHE_DIR
        / f"{digest}.json"
    )


def fetch_transaction(
    signature: str,
) -> dict[str, Any] | None:

    cache = tx_cache_path(
        signature
    )

    if cache.exists():
        try:
            return json.loads(
                cache.read_text(
                    encoding="utf-8"
                )
            )
        except Exception:
            pass

    result = solana_rpc(
        "getTransaction",
        [
            signature,
            {
                "encoding": "jsonParsed",
                "commitment": "confirmed",
                "maxSupportedTransactionVersion": 0,
            },
        ],
    )

    if result is None:
        return None

    try:
        cache.write_text(
            json.dumps(
                result,
                separators=(
                    ",",
                    ":",
                ),
            ),
            encoding="utf-8",
        )
    except Exception:
        pass

    return result


# ============================================================
# FETCH PROGRAM SIGNATURES (TARGETED, not bulk scan)
# ============================================================

def get_signatures_for_migration_window(
    target_before_ts: int,
    stop_after_ts: int,
) -> list[dict[str, Any]]:

    """
    Fetch Pump.fun transaction signatures in the requested window.
    Uses targeted RPC calls instead of bulk program scanning.
    
    Returns signatures of transactions that may contain migration instructions.
    """

    print()
    print("=" * 50)
    print("FETCHING PUMP.FUN TRANSACTION SIGNATURES")
    print("=" * 50)

    all_signatures: list[
        dict[str, Any]
    ] = []

    before: str | None = None

    pages = 0

    reached_start = False

    # Targeted approach: fetch limited pages instead of bulk scan
    # Public RPC rate limits at ~10 requests/minute, so we be conservative
    max_pages = 5  # ~500 signatures at page size 100

    while pages < max_pages:

        pages += 1

        config: dict[str, Any] = {
            "limit": SIGNATURE_PAGE_SIZE,
        }

        if before:
            config["before"] = before

        try:
            page = solana_rpc(
                "getSignaturesForAddress",
                [
                    PUMP_PROGRAM,
                    config,
                ],
            )
        except Exception as exc:
            print(
                f"RPC error on page {pages}: {exc}"
            )
            break

        if not page:
            print(
                "RPC returned no more signatures."
            )
            break

        print(
            f"Page {pages}: "
            f"{len(page)} signatures"
        )

        for item in page:

            signature = item.get(
                "signature"
            )

            block_time = item.get(
                "blockTime"
            )

            if block_time is None:
                continue

            block_time = int(
                block_time
            )

            # We only need the requested interval.
            if (
                stop_after_ts
                <= block_time
                <= target_before_ts
            ):
                all_signatures.append(
                    item
                )

        # Delay between pages to respect rate limits
        time.sleep(
            RPC_DELAY
        )

        before = page[-1][
            "signature"
        ]

        if len(all_signatures) >= MAX_GRADUATIONS * 5:
            # Safety cap for public RPC.
            print(
                "Safety cap reached."
            )
            break

    print()
    print(
        f"Relevant Pump signatures: "
        f"{len(all_signatures)}"
    )

    return all_signatures


# ============================================================
# VERIFY MIGRATIONS
# ============================================================

def find_migrations(
    signatures: list[dict[str, Any]],
) -> list[dict[str, Any]]:

    print()
    print("=" * 50)
    print("VERIFYING PUMP.FUN MIGRATIONS")
    print("=" * 50)

    migrations: dict[
        str,
        dict[str, Any],
    ] = {}

    total = len(
        signatures
    )

    # Sequential on purpose:
    # public RPC is the bottleneck and
    # this avoids hammering it.
    for index, item in enumerate(
        signatures,
        1,
    ):

        signature = item[
            "signature"
        ]

        block_time = item.get(
            "blockTime"
        )

        if block_time is None:
            continue

        if index == 1 or index % 20 == 0:
            print(
                f"[{index}/{total}] "
                f"checking transactions..."
            )

        try:
            tx = fetch_transaction(
                signature
            )
        except Exception as exc:
            print(
                f"  tx error: {exc}"
            )
            continue

        if not tx:
            continue

        message = (
            tx.get("transaction", {})
            .get("message", {})
        )

        instructions = (
            message.get(
                "instructions",
                [],
            )
            or []
        )

        found_migrate = False

        for instruction in instructions:

            if instruction_is_migrate(
                instruction
            ):
                found_migrate = True
                break

        if not found_migrate:
            # A transaction can also contain
            # a migrate instruction in inner
            # instructions.
            meta = tx.get(
                "meta"
            ) or {}

            inner = (
                meta.get(
                    "innerInstructions",
                    [],
                )
                or []
            )

            for group in inner:

                for instruction in (
                    group.get(
                        "instructions",
                        [],
                    )
                    or []
                ):

                    if instruction_is_migrate(
                        instruction
                    ):
                        found_migrate = True
                        break

                if found_migrate:
                    break

        if not found_migrate:
            continue

        mints = get_mint_candidates(
            tx
        )

        if not mints:
            continue

        # Usually exactly one "...pump" mint
        # belongs to the migration.
        mint = mints[0]

        if mint in migrations:
            continue

        migrations[mint] = {
            "mint": mint,
            "graduation_time": ts_to_iso(
                int(block_time)
            ),
            "timestamp_unix_secs": int(
                block_time
            ),
            "signature": signature,
            "slot": item.get(
                "slot",
                "",
            ),
        }

        print(
            f"  FOUND: {mint} "
            f"{ts_to_iso(int(block_time))}"
        )

        if len(migrations) >= MAX_GRADUATIONS:
            break

    result = list(
        migrations.values()
    )

    result.sort(
        key=lambda x: x[
            "timestamp_unix_secs"
        ]
    )

    print()
    print(
        f"Real migrations found: "
        f"{len(result)}"
    )

    return result


# ============================================================
# GECKOTERMINAL
# ============================================================

def gecko_get(
    path: str,
    params: dict[str, Any] | None = None,
) -> dict[str, Any]:

    url = (
        GECKO_BASE
        + path
    )

    if params:
        url += "?"
        url += urllib.parse.urlencode(
            params
        )

    return http_json(
        url,
        retries=5,
    )


# ============================================================
# FIND PUMPSWAP POOL
# ============================================================

def pool_cache_path(
    mint: str,
) -> Path:

    digest = hashlib.sha256(
        mint.encode()
    ).hexdigest()

    return (
        POOL_CACHE_DIR
        / f"{digest}.json"
    )


def get_pumpswap_pool(
    mint: str,
) -> str | None:

    cache = pool_cache_path(
        mint
    )

    if cache.exists():

        try:
            cached = json.loads(
                cache.read_text(
                    encoding="utf-8"
                )
            )

            if cached.get(
                "pool"
            ):
                return cached[
                    "pool"
                ]

        except Exception:
            pass

    # GeckoTerminal: find pools for token
    path = (
        f"/networks/solana/"
        f"tokens/{mint}/pools"
    )

    try:
        data = gecko_get(
            path,
            {
                "page": 1,
            },
        )
    except Exception as exc:
        print(
            f"  Gecko pool lookup error: "
            f"{exc}"
        )
        return None

    pools = (
        data.get("data", [])
        or []
    )

    candidates = []

    for pool in pools:

        attributes = (
            pool.get(
                "attributes"
            )
            or {}
        )

        relationships = (
            pool.get(
                "relationships"
            )
            or {}
        )

        dex = (
            relationships
            .get("dex")
            or {}
            .get("data")
            or {}
        )

        dex_id = dex.get(
            "id",
            ""
        )

        address = (
            attributes.get(
                "address"
            )
        )

        if not address:
            continue

        # Strong preference for PumpSwap.
        score = 0

        if (
            "pumpswap"
            in dex_id.lower()
        ):
            score += 100

        name = str(
            attributes.get(
                "name",
                ""
            )
        ).lower()

        if "pump" in name:
            score += 10

        reserve = safe_float(
            attributes.get(
                "reserve_in_usd"
            )
        )

        if reserve:
            score += min(
                reserve / 100000.0,
                20,
            )

        candidates.append(
            (
                score,
                address,
                dex_id,
                name,
            )
        )

    if not candidates:
        return None

    candidates.sort(
        reverse=True,
        key=lambda x: x[0],
    )

    best = candidates[0]

    result = {
        "pool": best[1],
        "dex_id": best[2],
        "name": best[3],
    }

    try:
        cache.write_text(
            json.dumps(result),
            encoding="utf-8",
        )
    except Exception:
        pass

    return best[1]


# ============================================================
# OHLC CACHE
# ============================================================

def ohlc_cache_path(
    pool: str,
    before_ts: int,
) -> Path:

    digest = hashlib.sha256(
        f"{pool}:{before_ts}".encode()
    ).hexdigest()

    return (
        OHLC_CACHE_DIR
        / f"{digest}.json"
    )


def get_ohlcv(
    pool: str,
    before_ts: int,
) -> list[list[Any]]:

    cache = ohlc_cache_path(
        pool,
        before_ts,
    )

    if cache.exists():

        try:
            return json.loads(
                cache.read_text(
                    encoding="utf-8"
                )
            )

        except Exception:
            pass

    path = (
        f"/networks/solana/"
        f"pools/{pool}/ohlcv/"
        f"{TIMEFRAME}"
    )

    # GeckoTerminal expects epoch seconds.
    params = {
        "aggregate": AGGREGATE,
        "before_timestamp": (
            before_ts
        ),
        "limit": 1000,
    }

    data = gecko_get(
        path,
        params,
    )

    raw = (
        data
        .get("data", {})
        .get("attributes", {})
        .get("ohlcv_list", [])
    )

    try:
        cache.write_text(
            json.dumps(raw),
            encoding="utf-8",
        )
    except Exception:
        pass

    return raw


# ============================================================
# BUILD CANDLE DATASET
# ============================================================

def download_candles(
    migrations: list[dict[str, Any]],
) -> list[dict[str, Any]]:

    print()
    print("=" * 60)
    print("DOWNLOADING REAL PUMPSWAP 1-MINUTE OHLCV")
    print("=" * 60)

    result = []

    for index, event in enumerate(
        migrations,
        1,
    ):

        mint = event[
            "mint"
        ]

        graduation_ts = event[
            "timestamp_unix_secs"
        ]

        print()
        print(
            f"[{index}/{len(migrations)}] "
            f"{mint}"
        )

        pool = get_pumpswap_pool(
            mint
        )

        if not pool:

            print(
                "  no PumpSwap pool found"
            )

            continue

        print(
            f"  pool: {pool}"
        )

        # Give the public API time to reset.
        time.sleep(
            GECKO_DELAY
        )

        try:
            candles = get_ohlcv(
                pool,
                graduation_ts
                + HORIZON_MINUTES * 60
                + 120,
            )

        except Exception as exc:

            print(
                f"  OHLC error: {exc}"
            )

            continue

        # GeckoTerminal returns newest first in many configurations.
        # Normalize to ascending.
        normalized = []

        for candle in candles:

            if not isinstance(
                candle,
                list,
            ):
                continue

            if len(candle) < 6:
                continue

            try:

                ts = int(
                    candle[0]
                )

                open_price = safe_float(
                    candle[1]
                )

                high = safe_float(
                    candle[2]
                )

                low = safe_float(
                    candle[3]
                )

                close = safe_float(
                    candle[4]
                )

                volume = safe_float(
                    candle[5]
                )

            except Exception:
                continue

            if any(
                x is None
                for x in (
                    open_price,
                    high,
                    low,
                    close,
                )
            ):
                continue

            # Only post-graduation window.
            if (
                ts
                < graduation_ts
            ):
                continue

            if (
                ts
                > graduation_ts
                + HORIZON_MINUTES * 60
            ):
                continue

            normalized.append(
                {
                    "mint": mint,
                    "pool": pool,
                    "graduation_timestamp": (
                        graduation_ts
                    ),
                    "timestamp_unix_secs": ts,
                    "time": ts_to_iso(
                        ts
                    ),
                    "open": open_price,
                    "high": high,
                    "low": low,
                    "close": close,
                    "volume": (
                        volume
                        if volume is not None
                        else 0.0
                    ),
                }
            )

        normalized.sort(
            key=lambda x: x[
                "timestamp_unix_secs"
            ]
        )

        print(
            f"  usable candles: "
            f"{len(normalized)}"
        )

        result.extend(
            normalized
        )

        if (
            index
            < len(migrations)
        ):
            time.sleep(
                GECKO_DELAY
            )

    fields = [
        "mint",
        "pool",
        "graduation_timestamp",
        "timestamp_unix_secs",
        "time",
        "open",
        "high",
        "low",
        "close",
        "volume",
    ]

    with open(
        CANDLES_FILE,
        "w",
        newline="",
        encoding="utf-8",
    ) as f:

        writer = csv.DictWriter(
            f,
            fieldnames=fields,
        )

        writer.writeheader()

        for row in result:
            writer.writerow(
                row
            )

    print()
    print(
        f"Saved: {CANDLES_FILE}"
    )

    return result


# ============================================================
# SAVE MIGRATIONS
# ============================================================

def save_migrations(
    migrations: list[dict[str, Any]],
) -> None:

    with open(
        GRADUATIONS_FILE,
        "w",
        newline="",
        encoding="utf-8",
    ) as f:

        fields = [
            "mint",
            "graduation_time",
            "timestamp_unix_secs",
            "signature",
            "slot",
        ]

        writer = csv.DictWriter(
            f,
            fieldnames=fields,
        )

        writer.writeheader()

        writer.writerows(
            migrations
        )


# ============================================================
# BACKTEST
# ============================================================

def group_candles(
    candles: list[dict[str, Any]],
) -> dict[
    str,
    list[dict[str, Any]]
]:

    grouped: dict[
        str,
        list[dict[str, Any]]
    ] = {}

    for candle in candles:

        grouped.setdefault(
            candle["mint"],
            [],
        ).append(
            candle
        )

    for rows in grouped.values():
        rows.sort(
            key=lambda x: x[
                "timestamp_unix_secs"
            ]
        )

    return grouped


def simulate_trade(
    event: dict[str, Any],
    candles: list[dict[str, Any]],
) -> dict[str, Any]:

    result = {
        "mint": event["mint"],
        "graduation_time": event[
            "graduation_time"
        ],
        "entry_price": "",
        "exit_price": "",
        "exit_time": "",
        "return_pct": "",
        "pnl_sol": "",
        "exit_reason": "",
        "candles_used": len(
            candles
        ),
    }

    if not candles:
        result["exit_reason"] = (
            "NO_DATA"
        )
        return result

    # Entry:
    # first complete candle after graduation.
    entry = candles[0]

    entry_price = safe_float(
        entry["open"]
    )

    if (
        entry_price is None
        or entry_price <= 0
    ):
        result["exit_reason"] = (
            "INVALID_ENTRY"
        )
        return result

    tp_price = (
        entry_price
        * (1.0 + TAKE_PROFIT)
    )

    sl_price = (
        entry_price
        * (1.0 - STOP_LOSS)
    )

    graduation_ts = event[
        "timestamp_unix_secs"
    ]

    last_close = entry_price

    exit_price = None
    exit_ts = None
    reason = "MAX_HOLD"

    for candle in candles:

        ts = candle[
            "timestamp_unix_secs"
        ]

        minutes = (
            ts - graduation_ts
        ) / 60.0

        if minutes > MAX_HOLD_MINUTES:
            break

        high = safe_float(
            candle["high"]
        )

        low = safe_float(
            candle["low"]
        )

        close = safe_float(
            candle["close"]
        )

        if any(
            x is None
            for x in (
                high,
                low,
                close,
            )
        ):
            continue

        last_close = close

        # OHLC cannot tell which happened first.
        # Conservative rule: SL wins if both hit in same candle.
        if (
            low <= sl_price
            and high >= tp_price
        ):

            exit_price = sl_price
            exit_ts = ts
            reason = (
                "STOP_AND_TP_SAME_CANDLE"
            )

            break

        if low <= sl_price:

            exit_price = sl_price
            exit_ts = ts
            reason = "STOP_LOSS"

            break

        if high >= tp_price:

            exit_price = tp_price
            exit_ts = ts
            reason = "TAKE_PROFIT"

            break

    if exit_price is None:

        exit_price = last_close

        exit_ts = candles[
            -1
        ]["timestamp_unix_secs"]

        reason = "MAX_HOLD"

    gross_return = (
        exit_price
        / entry_price
        - 1.0
    )

    net_return = (
        gross_return
        - FEE_RATE
    )

    pnl = (
        POSITION_SIZE_SOL
        * net_return
    )

    result.update(
        {
            "entry_price": entry_price,
            "exit_price": exit_price,
            "exit_time": ts_to_iso(
                exit_ts
            ),
            "return_pct": (
                net_return * 100.0
            ),
            "pnl_sol": pnl,
            "exit_reason": reason,
        }
    )

    return result


def run_backtest(
    migrations: list[dict[str, Any]],
    candles: list[dict[str, Any]],
) -> None:

    print()
    print("=" * 60)
    print("BACKTEST")
    print("=" * 60)

    grouped = group_candles(
        candles
    )

    trades = []

    for migration in migrations:

        mint = migration[
            "mint"
        ]

        trade = simulate_trade(
            migration,
            grouped.get(
                mint,
                [],
            ),
        )

        trades.append(
            trade
        )

    valid = [
        x
        for x in trades
        if isinstance(
            x["pnl_sol"],
            (int, float),
        )
    ]

    with open(
        BACKTEST_FILE,
        "w",
        newline="",
        encoding="utf-8",
    ) as f:

        fields = [
            "mint",
            "graduation_time",
            "entry_price",
            "exit_price",
            "exit_time",
            "return_pct",
            "pnl_sol",
            "exit_reason",
            "candles_used",
        ]

        writer = csv.DictWriter(
            f,
            fieldnames=fields,
        )

        writer.writeheader()
        writer.writerows(
            trades
        )

    if not valid:

        text = (
            "No valid trades with price data.\n"
        )

        SUMMARY_FILE.write_text(
            text,
            encoding="utf-8",
        )

        print(
            text
        )

        return

    pnl = [
        float(
            x["pnl_sol"]
        )
        for x in valid
    ]

    returns = [
        float(
            x["return_pct"]
        )
        for x in valid
    ]

    wins = [
        x
        for x in pnl
        if x > 0
    ]

    losses = [
        x
        for x in pnl
        if x < 0
    ]

    total_pnl = sum(
        pnl
    )

    avg_pnl = (
        total_pnl
        / len(pnl)
    )

    avg_return = (
        sum(returns)
        / len(returns)
    )

    win_rate = (
        len(wins)
        / len(valid)
        * 100.0
    )

    gross_profit = sum(
        wins
    )

    gross_loss = abs(
        sum(losses)
    )

    if gross_loss > 0:
        profit_factor = (
            gross_profit
            / gross_loss
        )
    else:
        profit_factor = float(
            "inf"
        )

    # Equity / drawdown.
    equity = INITIAL_CAPITAL_SOL
    peak = equity
    max_drawdown = 0.0

    for value in pnl:

        equity += value

        peak = max(
            peak,
            equity,
        )

        drawdown = (
            peak
            - equity
        )

        max_drawdown = max(
            max_drawdown,
            drawdown,
        )

    exit_counts: dict[
        str,
        int
    ] = {}

    for trade in valid:

        reason = str(
            trade[
                "exit_reason"
            ]
        )

        exit_counts[
            reason
        ] = (
            exit_counts.get(
                reason,
                0,
            )
            + 1
        )

    lines = [
        "Pump.fun Graduation Strategy",
        "",
        f"Period: {START_DATE} -> {END_DATE}",
        f"Migrations found: {len(migrations)}",
        f"Trades with price data: {len(valid)}",
        "",
        f"Initial capital: "
        f"{INITIAL_CAPITAL_SOL:.6f} SOL",
        f"Position size: "
        f"{POSITION_SIZE_SOL:.6f} SOL",
        "",
        f"Take profit: "
        f"+{TAKE_PROFIT * 100:.2f}%",
        f"Stop loss: "
        f"-{STOP_LOSS * 100:.2f}%",
        f"Max hold: "
        f"{MAX_HOLD_MINUTES} min",
        f"Fee model: "
        f"{FEE_RATE * 100:.3f}%",
        "",
        f"Total PnL: "
        f"{total_pnl:.6f} SOL",
        f"Average PnL/trade: "
        f"{avg_pnl:.6f} SOL",
        f"Average return/trade: "
        f"{avg_return:.4f}%",
        f"Win rate: "
        f"{win_rate:.2f}%",
        f"Profit factor: "
        f"{profit_factor:.4f}",
        f"Max drawdown: "
        f"{max_drawdown:.6f} SOL",
        f"Final equity: "
        f"{equity:.6f} SOL",
        "",
        "Exit reasons:",
    ]

    for reason, count in sorted(
        exit_counts.items()
    ):
        lines.append(
            f"  {reason}: {count}"
        )

    text = "\n".join(
        lines
    )

    print()
    print(text)

    SUMMARY_FILE.write_text(
        text,
        encoding="utf-8",
    )

    print()
    print(
        f"Saved: {BACKTEST_FILE}"
    )

    print(
        f"Saved: {SUMMARY_FILE}"
    )


# ============================================================
# MAIN
# ============================================================

def main() -> int:

    ensure_dirs()

    print()
    print("=" * 70)
    print("PUMPFUN PUBLIC-DATA HISTORICAL BACKTEST")
    print("=" * 70)

    print()
    print("RPC:", RPC_URL)

    print(
        "Period:",
        START_DATE,
        "->",
        END_DATE,
    )

    print(
        "No Bitquery/API key required."
    )

    start_ts = date_to_ts(
        START_DATE
    )

    end_ts = date_to_ts(
        END_DATE
    )

    if start_ts >= end_ts:
        print(
            "ERROR: START_DATE must be "
            "before END_DATE."
        )
        return 1

    # --------------------------------------------------------
    # 1. Signatures
    # --------------------------------------------------------

    print()
    print("Fetching Pump.fun transaction signatures...")

    signatures = (
        get_signatures_for_migration_window(
            end_ts,
            start_ts,
        )
    )

    if not signatures:
        print()
        print(
            "No Pump.fun transactions "
            "were found in the requested interval."
        )
        return 2

    print(
        f"Found {len(signatures)} signatures in window."
    )

    # --------------------------------------------------------
    # 2. True migrate transactions
    # --------------------------------------------------------

    print()
    print("Verifying Pump.fun migrate transactions...")

    migrations = (
        find_migrations(
            signatures
        )
    )

    if not migrations:
        print()
        print(
            "No verified migrate transactions found."
        )
        return 3

    save_migrations(
        migrations
    )

    print()
    print(
        f"Saved: {GRADUATIONS_FILE}"
    )

    print(
        f"Real migrations found: {len(migrations)}"
    )

    # --------------------------------------------------------
    # 3. Real PumpSwap OHLCV
    # --------------------------------------------------------

    print()
    print("Downloading real PumpSwap OHLCV data from GeckoTerminal...")

    candles = (
        download_candles(
            migrations
        )
    )

    print()
    print(
        f"Saved: {CANDLES_FILE}"
    )

    print(
        f"Total candles downloaded: {len(candles)}"
    )

    # --------------------------------------------------------
    # 4. Local backtest
    # --------------------------------------------------------

    print()
    print("Running backtest...")

    run_backtest(
        migrations,
        candles,
    )

    print()
    print("=" * 70)
    print("DONE")
    print("=" * 70)

    return 0


if __name__ == "__main__":
    sys.exit(
        main()
    )