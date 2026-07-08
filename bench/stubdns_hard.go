// stubdns_hard: same instant authoritative stub, but hardened against loopback
// UDP drops — N SO_REUSEPORT sockets, each with 8MB SO_RCVBUF/SO_SNDBUF. Used to
// test whether mosdns's ~1s forward retransmits are stub-side drops (they vanish
// here) or mosdns-side (they persist even with a drop-proof stub).
package main

import (
	"context"
	"encoding/binary"
	"net"
	"os"
	"runtime"
	"syscall"
)

func control(network, address string, c syscall.RawConn) error {
	return c.Control(func(fd uintptr) {
		syscall.SetsockoptInt(int(fd), syscall.SOL_SOCKET, syscall.SO_REUSEADDR, 1)
		syscall.SetsockoptInt(int(fd), syscall.SOL_SOCKET, syscall.SO_REUSEPORT, 1)
		syscall.SetsockoptInt(int(fd), syscall.SOL_SOCKET, syscall.SO_RCVBUF, 8<<20)
		syscall.SetsockoptInt(int(fd), syscall.SOL_SOCKET, syscall.SO_SNDBUF, 8<<20)
	})
}

func serve(pc *net.UDPConn) {
	ip := net.IP{93, 184, 216, 34}
	buf := make([]byte, 1500)
	for {
		n, peer, err := pc.ReadFromUDP(buf)
		if err != nil || n < 12 {
			continue
		}
		q := buf[:n]
		i := 12
		for i < n && q[i] != 0 {
			i += int(q[i]) + 1
		}
		qtypeOff := i + 1
		var qtype uint16
		if qtypeOff+2 <= n {
			qtype = binary.BigEndian.Uint16(q[qtypeOff : qtypeOff+2])
		}
		qEnd := qtypeOff + 4
		if qEnd > n {
			qEnd = n
		}
		resp := make([]byte, qEnd, qEnd+16)
		copy(resp, q[:qEnd])
		resp[2] = 0x84
		resp[3] = 0x80
		binary.BigEndian.PutUint16(resp[4:6], 1)
		var anc uint16
		if qtype == 1 {
			anc = 1
		}
		binary.BigEndian.PutUint16(resp[6:8], anc)
		binary.BigEndian.PutUint16(resp[8:10], 0)
		binary.BigEndian.PutUint16(resp[10:12], 0)
		if anc == 1 {
			resp = append(resp, 0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, ip[0], ip[1], ip[2], ip[3])
		}
		pc.WriteToUDP(resp, peer)
	}
}

func main() {
	addr := "127.0.0.1:5300"
	if len(os.Args) > 1 {
		addr = os.Args[1]
	}
	lc := net.ListenConfig{Control: control}
	n := runtime.NumCPU()
	os.Stderr.WriteString("stubdns_hard: " + addr + " with reuseport sockets\n")
	for i := 1; i < n; i++ {
		p, err := lc.ListenPacket(context.Background(), "udp", addr)
		if err != nil {
			panic(err)
		}
		go serve(p.(*net.UDPConn))
	}
	p, err := lc.ListenPacket(context.Background(), "udp", addr)
	if err != nil {
		panic(err)
	}
	serve(p.(*net.UDPConn))
}
