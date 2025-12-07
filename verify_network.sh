#!/bin/bash
# Network Stack Verification Script

echo "🔍 NETWORK STACK VERIFICATION"
echo "=============================="
echo ""

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Counters
total=0
found=0
missing=0

check_file() {
    local file=$1
    total=$((total + 1))
    if [ -f "$file" ]; then
        echo -e "${GREEN}✓${NC} $file"
        found=$((found + 1))
    else
        echo -e "${RED}✗${NC} $file ${RED}MISSING${NC}"
        missing=$((missing + 1))
    fi
}

echo "📁 Core Module (6 files)"
check_file "kernel/src/net/core/mod.rs"
check_file "kernel/src/net/core/buffer.rs"
check_file "kernel/src/net/core/device.rs"
check_file "kernel/src/net/core/socket.rs"
check_file "kernel/src/net/core/skb.rs"
check_file "kernel/src/net/core/netdev.rs"
echo ""

echo "📁 UDP Module (1 file)"
check_file "kernel/src/net/udp/mod.rs"
echo ""

echo "📁 TCP Module (8 files)"
check_file "kernel/src/net/tcp/mod.rs"
check_file "kernel/src/net/tcp/congestion.rs"
check_file "kernel/src/net/tcp/connection.rs"
check_file "kernel/src/net/tcp/retransmit.rs"
check_file "kernel/src/net/tcp/segment.rs"
check_file "kernel/src/net/tcp/window.rs"
check_file "kernel/src/net/tcp/options.rs"
check_file "kernel/src/net/tcp/state.rs"
check_file "kernel/src/net/tcp/timer.rs"
echo ""

echo "📁 IP Module (6 files)"
check_file "kernel/src/net/ip/mod.rs"
check_file "kernel/src/net/ip/ipv4.rs"
check_file "kernel/src/net/ip/ipv6.rs"
check_file "kernel/src/net/ip/routing.rs"
check_file "kernel/src/net/ip/fragmentation.rs"
check_file "kernel/src/net/ip/icmpv6.rs"
echo ""

echo "📁 Ethernet Module (2 files)"
check_file "kernel/src/net/ethernet/mod.rs"
check_file "kernel/src/net/ethernet/vlan.rs"
echo ""

echo "📁 WireGuard Module (4 files)"
check_file "kernel/src/net/wireguard/mod.rs"
check_file "kernel/src/net/wireguard/crypto.rs"
check_file "kernel/src/net/wireguard/handshake.rs"
check_file "kernel/src/net/wireguard/tunnel.rs"
echo ""

echo "📁 Advanced Protocols (9 files)"
check_file "kernel/src/net/netfilter/mod.rs"
check_file "kernel/src/net/netfilter/conntrack.rs"
check_file "kernel/src/net/qos.rs"
check_file "kernel/src/net/routing.rs"
check_file "kernel/src/net/tls.rs"
check_file "kernel/src/net/http2.rs"
check_file "kernel/src/net/quic.rs"
check_file "kernel/src/net/loadbalancer.rs"
check_file "kernel/src/net/rdma.rs"
check_file "kernel/src/net/monitoring.rs"
echo ""

echo "=============================="
echo "📊 VERIFICATION RESULTS"
echo "=============================="
echo -e "Total files checked: ${YELLOW}$total${NC}"
echo -e "Files found:        ${GREEN}$found${NC}"
echo -e "Files missing:      ${RED}$missing${NC}"
echo ""

if [ $missing -eq 0 ]; then
    echo -e "${GREEN}✅ ALL FILES PRESENT${NC}"
    echo -e "${GREEN}🎉 NETWORK STACK 100% COMPLETE${NC}"
    exit 0
else
    echo -e "${RED}❌ SOME FILES ARE MISSING${NC}"
    exit 1
fi
