#!/usr/bin/env python3
"""Exercise two real DLNA receivers through one fnOS gateway using null audio.

Usage: python3 fnos/tools/test-speaker-instances.py /path/to/fusionplay-fnos
Run on a Linux host with a LAN interface and receiver discovery ports available.
"""
import argparse
import io
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen
import wave
from xml.etree import ElementTree as ET
from xml.sax.saxutils import escape


def await_value(label, query, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = query()
            if result:
                return result
        except (URLError, ConnectionError) as error:
            result = str(error)
        time.sleep(0.15)
    raise AssertionError(f"{label} timed out (last result {result!r})")


def fetch(url, data=None, method=None):
    headers = {"Content-Type": "application/json"}
    body = None if data is None else json.dumps(data).encode()
    with urlopen(Request(url, body, headers, method=method), timeout=3) as response:
        return json.load(response)


AVT = "urn:schemas-upnp-org:service:AVTransport:1"


def soap(port, action, **arguments):
    args = "".join(f"<{k}>{escape(str(v))}</{k}>" for k, v in arguments.items())
    message = (f'<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>'
               f'<u:{action} xmlns:u="{AVT}"><InstanceID>0</InstanceID>{args}</u:{action}>'
               '</s:Body></s:Envelope>')
    request = Request(f"http://127.0.0.1:{port}/upnp/control/avtransport",
                      message.encode(), {"SOAPAction": f'"{AVT}#{action}"',
                                         "Content-Type": 'text/xml; charset="utf-8"'})
    with urlopen(request, timeout=3) as response:
        return ET.fromstring(response.read())


def wav_data():
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as audio:
        audio.setparams((1, 2, 24000, 0, "NONE", ""))
        audio.writeframes(bytes(48000 * 30))
    return buffer.getvalue()


class Track(BaseHTTPRequestHandler):
    content = wav_data()

    def log_message(self, *args):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "audio/wav")
        self.send_header("Content-Length", str(len(self.content)))
        self.end_headers()
        try:
            for i in range(0, len(self.content), 4800):
                self.wfile.write(self.content[i:i + 4800])
                time.sleep(0.05)
        except (ConnectionError, OSError):
            pass


def verify(base, media_port, root):
    def endpoint(room, path):
        return f"{base}/api/instances/{room}/{path}"

    def status(room):
        return fetch(endpoint(room, "status"))

    rooms = {"default": "Living room", "study": "Study"}
    instances = await_value("instance directory", lambda: fetch(base + "/api/instances"))
    assert {row["id"]: row["name"] for row in instances["instances"]} == rooms, instances
    # IPv6 is optional on NAS kernels. When loopback IPv6 is available,
    # both address families must reach each speaker's advertised RTSP port.
    addresses = ["127.0.0.1"]
    try:
        with socket.socket(socket.AF_INET6) as probe:
            probe.bind(("::1", 0))
        addresses.append("::1")
    except OSError:
        pass
    ports, ids = {}, set()
    for room, name in rooms.items():
        def ready():
            snapshot = status(room)
            return snapshot if all(p["state"] == "waiting" for p in snapshot["protocols"].values()) else None

        snapshot = await_value(f"{room} all three receivers ready", ready)
        for address in addresses:
            with socket.create_connection((address, snapshot["protocols"]["airplay"]["port"]), timeout=3) as rtsp:
                rtsp.sendall(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n")
                with rtsp.makefile("rb") as response:
                    assert response.readline().startswith(b"RTSP/1.0 200"), (room, address)
        ports[room] = snapshot["protocols"]["dlna"]["port"]
        with urlopen(f"http://127.0.0.1:{ports[room]}/description.xml", timeout=3) as desc:
            device = ET.fromstring(desc.read())
        assert device.findtext(".//{*}friendlyName") == name
        ids.add(device.findtext(".//{*}UDN"))
        title = f"Track for {name}"
        meta = ('<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" '
                'xmlns:dc="http://purl.org/dc/elements/1.1/"><item>'
                f'<dc:title>{title}</dc:title></item></DIDL-Lite>')
        soap(ports[room], "SetAVTransportURI",
             CurrentURI=f"http://127.0.0.1:{media_port}/{room}.wav", CurrentURIMetaData=meta)
        soap(ports[room], "Play", Speed=1)
    assert len(set(ports.values())) == len(ids) == 2 and None not in ids
    for room, name in rooms.items():
        def playing():
            snapshot = status(room)
            return (snapshot["current_source"] == "dlna"
                    and snapshot["media"]["title"] == f"Track for {name}"
                    and snapshot["protocols"]["dlna"]["state"] == "playing")
        await_value(f"{room} scoped media and decoded playback", playing)

    command = {"source": "dlna", "command": "play_pause"}
    assert fetch(endpoint("study", "player"), command)["accepted"]

    def transport(room):
        return soap(ports[room], "GetTransportInfo").findtext(".//CurrentTransportState")

    await_value("study pause", lambda: transport("study") == "PAUSED_PLAYBACK")
    assert transport("default") == "PLAYING"
    try:
        fetch(endpoint("missing", "player"), command)
    except HTTPError as error:
        assert error.code == 404, error
    else:
        raise AssertionError("missing instance accepted a command")
    # The filename is deliberately identical in both state directories: these
    # reads test that a cached artwork path cannot leak across rooms.
    for room in rooms:
        state = root if room == "default" else root / "instances" / room
        artwork = state / "artwork"
        artwork.mkdir(exist_ok=True)
        filename = "cover-art-aaaaaaaaaaaaaaaa.jpg"
        (artwork / filename).write_bytes(room.encode())
        with urlopen(endpoint(room, "artwork/" + filename), timeout=3) as response:
            assert response.read() == room.encode()

    # Moving the main speaker out of the primary receiver must not leave an
    # extra silent player. An old link must never control the remaining room.
    config = fetch(base + "/api/config")
    for device in config["audio"]["devices"]:
        device["enabled"] = False
    fetch(base + "/api/config", config, "PUT")
    catalog = fetch(base + "/api/instances")
    assert catalog["default"] == "study"
    assert [room["id"] for room in catalog["instances"]] == ["study"]
    try:
        fetch(endpoint("default", "player"), command)
    except HTTPError as error:
        assert error.code == 404, error
    else:
        raise AssertionError("dormant default intercepted a command")
    assert transport("study") == "PAUSED_PLAYBACK"
    print("PASS: two identities, six ready receivers, concurrent DLNA titles, "
          "isolated pause and artwork, dormant receiver routing")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp, ThreadingHTTPServer(("127.0.0.1", 0), Track) as media:
        threading.Thread(target=media.serve_forever, daemon=True).start()
        root = Path(tmp)
        config = {"receiver": {"name": "Living room"}, "audio": {
            "devices": [{"id": "hw:CARD=main,DEV=0", "name": "Main", "enabled": True},
                        {"id": "hw:CARD=study,DEV=0", "name": "Study DAC", "enabled": False}],
            "instances": [{"id": "study", "name": "Study", "device_id": "hw:CARD=study,DEV=0"}]}}
        (root / "config.json").write_text(json.dumps(config))
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        prefix = "/app/fusionplay-fnos"
        env = {**os.environ, "FUSIONPLAY_AUDIO_BACKEND": "null", "GATEWAY_PREFIX": prefix}
        env.pop("GATEWAY_SOCKET", None)
        with (root / "stderr").open("w+") as log:
            process = subprocess.Popen([str(args.binary.resolve()), "--config", str(root / "config.json"),
                                        "--state-dir", tmp, "--listen-tcp", f"127.0.0.1:{port}"],
                                       env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                verify(f"http://127.0.0.1:{port}{prefix}", media.server_port, root)
            except Exception:
                log.seek(0)
                print(log.read()[-5000:])
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(10)
                finally:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                media.shutdown()


if __name__ == "__main__":
    main()
