package com.airplayreceiver.desktop.backend

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LyricsParserTest {
    @Test
    fun parsesPlainLrcLines() {
        val lyrics = LrcParser.parse(
            """
            [00:01.00]First line
            [00:02.50]Second line
            """.trimIndent(),
        )
        assertNotNull(lyrics)
        lyrics!!
        assertTrue(lyrics.isSynchronized)
        assertEquals(listOf(1_000L, 2_500L), lyrics.lines.map { it.startMs })
        assertEquals(listOf("First line", "Second line"), lyrics.lines.map { it.text })
        assertTrue(lyrics.lines.all { it.words.isEmpty() })
    }

    @Test
    fun parsesEnhancedLrcWordTimestamps() {
        val lyrics = LrcParser.parse(
            "[00:01.00]<00:01.00>Hello <00:01.50>world",
        )
        assertNotNull(lyrics)
        val line = lyrics!!.lines.single()
        assertEquals(1_000L, line.startMs)
        assertEquals("Hello world", line.text)
        assertEquals(listOf(1_000L, 1_500L), line.words.map { it.startMs })
        assertEquals(1_500L, line.words[0].endMs)
        assertEquals(listOf("Hello ", "world"), line.words.map { it.text })
    }

    @Test
    fun parsesYrcTokenStream() {
        val lyrics = LrcParser.parse(
            """
            [00:01.000]{Hello }[00:01.500]{world}
            [00:03.000]{Next line}
            """.trimIndent(),
        )
        assertNotNull(lyrics)
        lyrics!!
        assertTrue(lyrics.isSynchronized)
        assertEquals(2, lyrics.lines.size)
        val first = lyrics.lines[0]
        assertEquals(1_000L, first.startMs)
        assertEquals("Hello world", first.text)
        assertEquals(listOf(1_000L, 1_500L), first.words.map { it.startMs })
        // Missing end times are inferred from the next word / next line.
        assertEquals(1_500L, first.words[0].endMs)
        assertEquals(3_000L, first.words[1].endMs)
        // Before the first word timestamp no word is active yet.
        assertEquals(-1, first.activeWordIndex(900) ?: -1)
        assertEquals(0, first.activeWordIndex(1_000) ?: -1)
        assertEquals(0, first.activeWordIndex(1_200) ?: -1)
        assertEquals(1, first.activeWordIndex(2_000) ?: -1)
        assertNull(lyrics.lines[1].activeWordIndex(500))
    }

    @Test
    fun untimedLyricsAreNotSynchronized() {
        val lyrics = LrcParser.parse("first\nsecond\n")
        assertNotNull(lyrics)
        assertFalse(lyrics!!.isSynchronized)
        assertEquals(listOf("first", "second"), lyrics.lines.map { it.text })
        assertTrue(lyrics.lines.all { it.startMs == null })
    }

    @Test
    fun appliesOffsetAndJoinsTranslationsAtTheSameTimestamp() {
        val lyrics = LrcParser.parse(
            """
            [offset:+500]
            [00:01.00]Original
            [00:01.00]Translation
            """.trimIndent(),
        )
        assertNotNull(lyrics)
        val line = lyrics!!.lines.single()
        assertEquals(1_500L, line.startMs)
        assertEquals("Original\nTranslation", line.text)
        assertTrue(line.words.isEmpty())
    }

    @Test
    fun ignoresMetadataAndEmptyLines() {
        val lyrics = LrcParser.parse(
            """
            [ar:Artist]
            [ti:Title]
            [00:02.00]Only line
            """.trimIndent(),
        )
        assertNotNull(lyrics)
        assertEquals(1, lyrics!!.lines.size)
        assertEquals("Only line", lyrics.lines.single().text)
    }

    @Test
    fun parsesQrcStyleMillisecondTimings() {
        val lyrics = LrcParser.parse(
            "[1000,2500]Hel(1000,500)lo (1500,500)world(2000,500)",
        )
        assertNotNull(lyrics)
        val line = lyrics!!.lines.single()
        assertEquals(1_000L, line.startMs)
        assertEquals("Hello world", line.text)
        assertEquals(listOf(1_000L, 1_500L, 2_000L), line.words.map { it.startMs })
        assertEquals(listOf(1_500L, 2_000L, 2_500L), line.words.map { it.endMs })
    }

    @Test
    fun serializesToFlutterStateMap() {
        val lyrics = LrcParser.parse("[00:01.00]<00:01.00>Hi <00:01.50>there")
        val map = lyrics!!.toStateMap()
        assertEquals("embedded", map["origin"])
        assertEquals(true, map["synced"])
        val lines = map["lines"] as List<*>
        val line = lines.single() as Map<*, *>
        assertEquals(1_000L, line["startMs"])
        val words = line["words"] as List<*>
        assertEquals(2, words.size)
        val firstWord = words[0] as Map<*, *>
        assertEquals(1_000L, firstWord["startMs"])
        assertEquals("Hi ", firstWord["text"])
    }
}
