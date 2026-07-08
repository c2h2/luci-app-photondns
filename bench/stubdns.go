// stubdns: a zero-latency authoritative DNS stub for benchmarking.
//
// It answers EVERY query on loopback in-memory with no I/O wait, so a resolver
// forwarding to it pays ~loopback RTT (microseconds) instead of real WAN RTT.
// Both resolvers-under-test forward here, so any stub cost is common-mode and
// cancels out of a photondns-vs-mosdns comparison. This is how we "remove the
// network delay": the upstream is effectively instantaneous.
//
//   A     -> NOERROR, one A record (93.184.216.34), TTL 60
//   AAAA  -> NOERROR, empty (no answer)
//   other -> NOERROR, empty
//
// Multiple goroutines share one UDP socket so the stub is never the bottleneck.
package main

import (
	"encoding/binary"
	"net"
	"os"
	"runtime"
)

func serve(pc *net.UDPConn, ansIP net.IP) {
	buf := make([]byte, 1500)
	for {
		n, peer, err := pc.ReadFromUDP(buf)
		if err != nil || n < 12 {
			continue
		}
		q := buf[:n]
		// walk the QNAME labels to find the qtype
		i := 12
		for i < n && q[i] != 0 {
			i += int(q[i]) + 1
		}
		qtypeOff := i + 1
		var qtype uint16
		if qtypeOff+2 <= n {
			qtype = binary.BigEndian.Uint16(q[qtypeOff : qtypeOff+2])
		}
		qEnd := qtypeOff + 4 // qtype(2) + qclass(2)
		if qEnd > n {
			qEnd = n
		}
		resp := make([]byte, qEnd, qEnd+16)
		copy(resp, q[:qEnd])
		resp[2] = 0x84 // QR=1, AA=1
		resp[3] = 0x80 // RA=1, rcode=0
		binary.BigEndian.PutUint16(resp[4:6], 1) // QDCOUNT
		var anc uint16
		if qtype == 1 { // A
			anc = 1
		}
		binary.BigEndian.PutUint16(resp[6:8], anc)
		binary.BigEndian.PutUint16(resp[8:10], 0)
		binary.BigEndian.PutUint16(resp[10:12], 0)
		if anc == 1 {
			resp = append(resp,
				0xc0, 0x0c, // name ptr -> question
				0x00, 0x01, // type A
				0x00, 0x01, // class IN
				0x00, 0x00, 0x00, 0x3c, // TTL 60
				0x00, 0x04, // rdlen 4
				ansIP[0], ansIP[1], ansIP[2], ansIP[3])
		}
		pc.WriteToUDP(resp, peer)
	}
}

func main() {
	addr := "127.0.0.1:5300"
	if len(os.Args) > 1 {
		addr = os.Args[1]
	}
	ua, err := net.ResolveUDPAddr("udp", addr)
	if err != nil {
		panic(err)
	}
	pc, err := net.ListenUDP("udp", ua)
	if err != nil {
		panic(err)
	}
	os.Stderr.WriteString("stubdns listening on " + addr + "\n")
	n := runtime.NumCPU()
	for k := 1; k < n; k++ {
		go serve(pc, net.IP{93, 184, 216, 34})
	}
	serve(pc, net.IP{93, 184, 216, 34})
}
