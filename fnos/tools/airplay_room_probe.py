"""Optional pyatv sender for the null-output multi-room integration test."""
import asyncio
import io
from ipaddress import IPv4Address
import plistlib
import socket
from unittest.mock import patch
import wave

import pyatv
from pyatv.conf import AppleTV, ManualService
from pyatv.const import Protocol
from pyatv.interface import MediaMetadata
from pyatv.settings import AirPlayVersion
from pyatv.storage.memory_storage import MemoryStorage
from pyatv.support.rtsp import RtspSession


def receiver_features(port):
    with socket.create_connection(("127.0.0.1", port), timeout=3) as conn, conn.makefile("rb") as reply:
        conn.sendall(b"GET /info RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        assert b" 200 " in reply.readline()
        length = 0
        for line in reply:
            if line == b"\r\n":
                break
            if line.lower().startswith(b"content-length:"):
                length = int(line.partition(b":")[2])
        return plistlib.loads(reply.read(length))["features"]


async def verify_airplay_rooms(status, rooms, dlna_room):
    audio = io.BytesIO()
    with wave.open(audio, "wb") as wav:
        wav.setparams((2, 2, 44100, 0, "NONE", ""))
        wav.writeframes(bytes(44100 * 4 * 20))
    clients, tasks = [], []
    send_metadata = RtspSession.set_metadata

    # This sender sends an opening credit followed by a title-as-lyric packet
    # on the same, normally paired RTSP session (NetEase field convention).
    async def send_credits_and_lyrics(rtsp, session, seq, timestamp, meta):
        await send_metadata(rtsp, session, seq, timestamp, meta)
        lyric = MediaMetadata(title=f"Line {meta.title}", artist=f"{meta.title} - {meta.artist}")
        return await send_metadata(rtsp, session, seq, timestamp, lyric)

    try:
        with patch.object(RtspSession, "set_metadata", send_credits_and_lyrics):
            for room in rooms:
                port = status(room)["protocols"]["airplay"]["port"]
                flags = receiver_features(port)
                features = f"{flags & 0xffffffff:#x},{flags >> 32:#x}"
                config = AppleTV(IPv4Address("127.0.0.1"), room)
                config.add_service(ManualService(room, Protocol.RAOP, port,
                                   {"ft": features, "md": "0,1,2", "cn": "0,1", "sr": "44100", "ch": "2", "ss": "16"}))
                storage = MemoryStorage()
                settings = await storage.get_settings(config)
                settings.protocols.raop.protocol_version = AirPlayVersion.V2
                client = await pyatv.connect(config, asyncio.get_running_loop(), protocol=Protocol.RAOP, storage=storage)
                clients.append(client)
                meta = MediaMetadata(title=f"Song {room}", artist=f"Artist {room}")
                tasks.append(asyncio.create_task(client.stream.stream_file(io.BytesIO(audio.getvalue()), metadata=meta)))

            deadline = asyncio.get_running_loop().time() + 15
            first_frames = {}
            while asyncio.get_running_loop().time() < deadline:
                playing = set()
                for room in rooms:
                    snapshot = status(room)
                    media = snapshot["media"]
                    if (snapshot["current_source"] == "airplay" and media["title"] == f"Song {room}"
                            and media["artist"] == f"Artist {room}" and media["lyrics_text"] == f"Line Song {room}"):
                        frames = snapshot["audio"]["master_frames"]
                        if frames > first_frames.setdefault(room, frames):
                            playing.add(room)
                if len(playing) == len(rooms):
                    other = status(dlna_room)
                    assert other["current_source"] == "dlna" and other["protocols"]["dlna"]["state"] == "playing"
                    assert other["media"]["lyrics_text"] == f"[00:01.20]Line {dlna_room}"
                    print("PASS: two encrypted AirPlay 2 streams with independent decoded frames, opening credits "
                          "and title-as-lyric packets alongside another room's DLNA stream.")
                    return
                for task in tasks:
                    if task.done():
                        task.result()
                await asyncio.sleep(.15)
            raise AssertionError("concurrent AirPlay 2 metadata or PCM did not reach the expected rooms")
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        for client in clients:
            pending = client.close()
            if pending:
                await asyncio.gather(*pending, return_exceptions=True)
