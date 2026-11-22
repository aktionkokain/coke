#include "../../crates/coke-host/include/coke_host.h"

_Static_assert(sizeof(void *) == 4, "x86 fixture");
_Static_assert(COKE_HOST_V1_SIZE == 24, "v1 prefix");
_Static_assert(COKE_HOST_V2_SIZE == 112, "v2 and v3 prefix");
_Static_assert(COKE_HOST_V2_1_SIZE == 124, "v2.1 prefix");
_Static_assert(COKE_HOST_V4_SIZE == 132, "v4 prefix");
_Static_assert(COKE_HOST_V6_SIZE == 144, "v5/v6 prefix");
_Static_assert(COKE_HOST_V7_SIZE == 152, "v7 prefix");
_Static_assert(COKE_HOST_V8_SIZE == 156, "v8 prefix");
_Static_assert(sizeof(CokeProfStats) == 216, "frozen profiler snapshot");
_Static_assert(offsetof(CokeProfStats, last_cpu_frame_ns) == 208, "profiler tail");
_Static_assert(sizeof(CokeRangeResult) == 44, "range result");

/* Example init for a C mod. v1 hosts do not provide the size field. */
int32_t COKE_CALL coke_init(const CokeHost *host)
{
    if (host == NULL || host->api_version < 2 || host->size < COKE_HOST_V2_SIZE) {
        return 0;
    }
    if (host->log != NULL) {
        host->log("Coke C ABI fixture");
    }
    return 1;
}
