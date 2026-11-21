#!/usr/bin/env python3
"""Browse real IPv4 LAN advertisements for two running fnOS speaker receivers.

Install dnslib, then run this from the NAS or a computer on its LAN:
  python3 fnos/tools/test-speaker-discovery.py <local-interface-ip> "客厅" "书房"
The chosen names must be those configured on the NAS. No playback is initiated.
"""
import argparse
import base64
import json
import select
import socket
import time
import xml.etree.ElementTree as ET
from urllib.parse import urlsplit
from urllib.request import ProxyHandler, build_opener

from dnslib import DNSError, DNSRecord, QTYPE


def query(local_ip, target, port, payloads, multicast_replies=False):
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        addr = socket.inet_aton(local_ip)
        sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, addr)
        if multicast_replies:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(('', port))
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, socket.inet_aton(target) + addr)
        else:
            sock.bind((local_ip, 0))
        for _ in range(2):
            for packet in payloads:
                sock.sendto(packet, (target, port))
            until = time.monotonic() + 2
            while select.select([sock], [], [], max(0, until - time.monotonic()))[0]:
                yield sock.recvfrom(65535)
                if time.monotonic() >= until:
                    break


def dns_answers(ip, services, multicast_replies):
    packets = [DNSRecord.question(name + '.local', 'PTR') for name in services]
    for packet in packets:
        packet.header.id = packet.header.rd = 0
    for wire, _ in query(ip, '224.0.0.251', 5353, [p.pack() for p in packets], multicast_replies):
        try:
            packet = DNSRecord.parse(wire)
        except DNSError:
            continue
        for rr in packet.rr + packet.ar:
            if packet.header.qr and rr.ttl:
                yield rr


def search(ip, names):
    found = {name: {} for name in names}
    for rr in dns_answers(ip, ['_airplay._tcp'], True):
        if not str(rr.rname).endswith('._airplay._tcp.local.'):
            continue
        name = rr.rname.label[0].decode('utf-8', 'replace')
        if name in found and rr.rtype == QTYPE.SRV:
            found[name]['airplay_rtsp_port'] = rr.rdata.port
        if name in found and rr.rtype == QTYPE.TXT:
            fields = dict(item.split(b'=', 1) for item in rr.rdata.data if b'=' in item)
            found[name]['airplay_device_id'] = fields.get(b'deviceid', b'').decode()

    for rr in dns_answers(ip, ['_mi-connect._udp', '_lyra-mdns._udp'], False):
        if rr.rtype != QTYPE.TXT:
            continue
        fields = dict(item.split(b'=', 1) for item in rr.rdata.data if b'=' in item)
        if str(rr.rname).endswith('._mi-connect._udp.local.') and b'name' in fields:
            name = fields[b'name'].decode()
            if name in found:
                found[name]['miplay_control_port'] = int.from_bytes(base64.b64decode(fields[b'appsData'])[-2:], 'big')
                found[name]['miplay_id'] = fields[b'idHash'].decode()
                found[name]['miplay_mac'] = fields.get(b'mac', b'').decode()
        if str(rr.rname).endswith('._lyra-mdns._udp.local.') and b'AppData' in fields:
            app_data = base64.b64decode(fields[b'AppData'])
            if len(app_data) > 23:
                name = app_data[24:24 + app_data[23]].decode('utf-8', 'replace')
                if name in found:
                    found[name]['lyra_udp_port'] = int.from_bytes(app_data[17:19], 'big')

    urn = 'urn:schemas-upnp-org:device:MediaRenderer:1'
    request = (f'M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\n'
               f'MAN: "ssdp:discover"\r\nMX: 1\r\nST: {urn}\r\n\r\n').encode()
    seen = set()
    opener = build_opener(ProxyHandler({}))
    for response, (peer, _) in query(ip, '239.255.255.250', 1900, [request]):
        location = next((line.partition(':')[2].strip() for line in response.decode('ascii', 'replace').splitlines()
                         if line.lower().startswith('location:')), '')
        target = urlsplit(location)
        if target.scheme != 'http' or target.hostname != peer or location in seen:
            continue
        seen.add(location)
        with opener.open(location, timeout=2) as reply:
            info = ET.fromstring(reply.read(65536))
        name = info.findtext('.//{*}friendlyName')
        if name in found:
            found[name]['dlna_udn'] = info.findtext('.//{*}UDN')
            found[name]['dlna_port'] = target.port
    return found


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('local_ip')
    parser.add_argument('names', nargs='+')
    args = parser.parse_args()
    if len(set(args.names)) < 2:
        parser.error('provide at least two distinct advertised speaker names')
    results = search(args.local_ip, args.names)
    print(json.dumps(results, indent=2, ensure_ascii=False))
    for field in ('airplay_rtsp_port', 'airplay_device_id', 'dlna_udn', 'dlna_port',
                  'miplay_control_port', 'miplay_id', 'miplay_mac', 'lyra_udp_port'):
        values = {fields.get(field) for fields in results.values()}
        if not all(values) or len(values) != len(results):
            raise SystemExit(f'Missing or duplicated {field}')
