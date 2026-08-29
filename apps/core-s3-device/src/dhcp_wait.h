/* SPDX-License-Identifier: GPL-3.0-only */

#ifndef DESKKIN_DHCP_WAIT_H
#define DESKKIN_DHCP_WAIT_H

#include <stdbool.h>

enum deskkin_dhcp_wait_decision {
	DESKKIN_DHCP_WAIT_CONTINUE,
	DESKKIN_DHCP_WAIT_READY,
	DESKKIN_DHCP_WAIT_CANCELLED,
	DESKKIN_DHCP_WAIT_TIMED_OUT,
};

static inline enum deskkin_dhcp_wait_decision
deskkin_dhcp_wait_decide(bool cancelled, bool has_preferred_address,
			 bool deadline_reached)
{
	if (cancelled) {
		return DESKKIN_DHCP_WAIT_CANCELLED;
	}
	if (has_preferred_address) {
		return DESKKIN_DHCP_WAIT_READY;
	}
	if (deadline_reached) {
		return DESKKIN_DHCP_WAIT_TIMED_OUT;
	}
	return DESKKIN_DHCP_WAIT_CONTINUE;
}

#endif
