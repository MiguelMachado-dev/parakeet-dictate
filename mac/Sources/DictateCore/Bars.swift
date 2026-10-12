// A port of src/parakeet_dictate/styles/bars.py: a smoked-glass capsule whose mirrored bars carry every phase.
// The Python's float32 NumPy math (Python float scalars cast to float32 at each operation, np.interp in float64) is
// kept in the same types and order, so the premultiplied bytes match it exactly.

import Foundation

public enum Bars {
    static let nBars = 31
    static let center: Float = 15  // (N_BARS - 1) / 2
    static let k: [Float] = (0..<nBars).map { abs(Float($0) - center) }
    static let kNorm: [Float] = k.map { $0 / center }
    static let pitch = 6.0
    static let barR = 1.5
    static let glowMargin = 8.5
    static let lMax = 14.5
    static let hopsPerBar: Float = 2.0
    static let capsuleW = 214.0, capsuleH = 46.0
    static let latchW = 14.0
    static let cy = 58.0

    static let glassTop = SIMD3<Float>(0.105, 0.115, 0.145)
    static let glassBottom = SIMD3<Float>(0.045, 0.052, 0.068)
    static let glassAlpha: Float = 0.9

    /// Positions and 0-255 colours.
    typealias Stops = [(Double, SIMD3<Float>)]

    static func ramp(_ stops: Stops, _ t: [Float]) -> [SIMD3<Float>] {
        let at = stops.map { $0.0 }
        let rgb = stops.map { $0.1 / 255 }
        let channels = (0..<3).map { i in rgb.map { Double($0[i]) } }
        return t.map { v in SIMD3((0..<3).map { Float(interp(Double(v), at, channels[$0])) }) }
    }

    // Budgerigar plumage, yellow face to green body to blue tail: fresh sound at the centre, cooling as it ages outward.
    // scripts/make_icon.py paints the app icon's bars with the same stops.
    static let plumageStops: Stops = [
        (0.0, [250, 255, 185]),
        (0.2, [205, 250, 110]),
        (0.47, [70, 228, 160]),
        (0.76, [38, 160, 205]),
        (1.0, [62, 84, 160]),
    ]
    static let plumage = ramp(plumageStops, kNorm)
    static let idleC = SIMD3<Float>(0.5, 0.72, 0.8)
    static let iris: Stops = [(0.0, [140, 92, 255]), (0.33, [225, 84, 255]), (0.66, [92, 140, 255]), (1.0, [140, 92, 255])]
    static let scan = SIMD3<Float>(0.86, 0.93, 1.0)
    static let doneC = SIMD3<Float>(0.38, 0.94, 0.62)
    static let cancelC = SIMD3<Float>(0.55, 0.57, 0.62)
    static let failC = SIMD3<Float>(1.0, 0.43, 0.43)
    static let latchC = SIMD3<Float>(0.9, 0.92, 0.96)

    static func ease(_ t: Double) -> Double {
        let t = min(1.0, max(0.0, t))
        return 1 - pow(1 - t, 3)
    }

    enum Glyph {
        case latch, check, alert
    }

    /// What the phase asks of the strip. Per-bar arrays run left to right; lengths are logical px.
    struct Pose {
        var halves: [Float]
        var colors: [SIMD3<Float>]
        var alpha: [Float]
        var aura: [Float]
        var flare = 0.0
        var width = 1.0
        var pitch = 1.0
        var shift = 0.0
        var rim: SIMD3<Float>? = nil
        var rimStrength = 0.0
        var glyph: Glyph? = nil
        var glyphT = 0.0
    }

    static func recording(_ frame: OverlayFrame) -> Pose {
        let levels = frame.levels.reversed().map(Double.init)
        let levelsAt = levels.indices.map(Double.init)
        let spectrum = frame.spectrum.map(Double.init)
        let spectrumAt = spectrum.indices.map(Double.init)
        let phase = Float(2 * Double.pi * 0.55 * frame.recordingElapsed)
        var pose = Pose(halves: [], colors: [], alpha: [], aura: [], flare: 0.6, glyph: frame.handsFree ? .latch : nil)
        for i in 0..<nBars {
            let history = Float(interp(Double(k[i] * hopsPerBar), levelsAt, levels))
            let voice = powf(history, 1.7) * (1 - 0.5 * sq(kNorm[i]))
            // Silence ripples outward the same way speech does, only a pixel tall.
            let breath = 0.5 + 0.5 * sinf(phase - k[i] * 0.55)
            let lit = clip(history * 2.5)
            // Speech energy sits in bands 3 to 20 (about 150 Hz to 5 kHz); the lowest of those breathe at the centre.
            let bands = interp(Double(3 + k[i] * Float(17 / Double(center))), spectrumAt, spectrum)
            pose.halves.append(max(1.2 * breath, voice * Float(lMax)))
            pose.colors.append(idleC + (plumage[i] - idleC) * lit)
            pose.alpha.append((1 - 0.45 * sq(kNorm[i])) * (0.55 + 0.45 * max(breath, lit)))
            pose.aura.append(Float(min(max(bands * 1.8, 0), 1)))
        }
        return pose
    }

    static func transcribing(_ frame: OverlayFrame) -> Pose {
        let t = frame.phaseElapsed
        let settle = ease(t / 0.15)
        let head = 1 - pyMod(t / 0.55, 1.0)
        let start = recording(frame)
        let settleF = Float(settle)
        var pose = Pose(halves: [], colors: [], alpha: [], aura: [], flare: start.flare * (1 - settle))
        for i in 0..<nBars {
            let bump = expf(-sq((kNorm[i] - Float(head)) / 0.16))
            let working = 1.0 + 7.0 * bump
            let scan = Self.scan * (0.55 + 0.45 * bump)
            pose.halves.append(start.halves[i] + (working - start.halves[i]) * settleF)
            pose.colors.append(start.colors[i] + (scan - start.colors[i]) * settleF)
            pose.alpha.append(start.alpha[i] + (1 - start.alpha[i]) * settleF)
            pose.aura.append(start.aura[i] * Float(1 - settle) + bump * 0.6 * settleF)
        }
        return pose
    }

    static func polishing(_ frame: OverlayFrame) -> Pose {
        let t = frame.phaseElapsed
        let settle = ease(t / 0.25)
        let settleF = Float(settle)
        let phaseA = Float(2 * Double.pi * 0.7 * t)
        let phaseB = Float(2 * Double.pi * 0.27 * t)
        let colors = ramp(iris, k.map { pyMod($0 / Float(Double(nBars) * 1.2) - Float(t * 0.35), 1.0) })
        var pose = Pose(halves: [], colors: [], alpha: [Float](repeating: 1, count: nBars), aura: [])
        for i in 0..<nBars {
            let a = 0.5 + 0.5 * sinf(phaseA - k[i] * 0.5)
            let b = 0.5 + 0.5 * sinf(phaseB + k[i] * 0.23 + 1.1)
            let working = 1.5 + 9.0 * a * (0.35 + 0.65 * b) * (1 - 0.45 * sq(kNorm[i]))
            pose.halves.append(1.0 + (working - 1.0) * settleF)
            pose.colors.append(scan + (colors[i] - scan) * settleF)
            pose.aura.append((0.25 + 0.5 * a * b) * settleF)
        }
        return pose
    }

    static func ending(_ frame: OverlayFrame, _ color: SIMD3<Float>, _ glyph: Glyph?) -> Pose {
        let t = frame.phaseElapsed
        let fall = ease(t / 0.14)
        let alpha: [Float] = k.map { k in
            if glyph != nil {
                let clear = clip((k - 3.0) / 2.0)
                return (clear + (1 - clear) * Float(1 - fall)) * Float(1 - 0.4 * fall)
            }
            // np.ones is float64, so without a glyph this is Double until the cast.
            let clear = 1.0
            return Float((clear + (1 - clear) * (1 - fall)) * (1 - 0.4 * fall))
        }
        return Pose(
            halves: [Float](repeating: Float(1.0 - fall), count: nBars),
            colors: [SIMD3<Float>](repeating: color, count: nBars),
            alpha: alpha,
            aura: [Float](repeating: 0, count: nBars),
            rim: color,
            rimStrength: exp(-t * 6.0),
            glyph: glyph,
            glyphT: t
        )
    }

    static func done(_ frame: OverlayFrame) -> Pose {
        var pose = ending(frame, doneC, .check)
        let front = Float(frame.phaseElapsed / 0.32)
        for i in 0..<nBars {
            let wave = expf(-sq((kNorm[i] - front) / 0.14)) * powf(1 - kNorm[i], 1.5)
            pose.halves[i] += 3.0 * wave
            pose.aura[i] = wave * 0.8
        }
        return pose
    }

    static func cancelled(_ frame: OverlayFrame) -> Pose {
        let fold = ease(frame.phaseElapsed / OverlayCanvas.beat)
        var pose = ending(frame, cancelC, nil)
        pose.pitch = 1 - 0.55 * fold
        pose.width = 1 - 0.4 * fold
        pose.rimStrength = 0.0
        return pose
    }

    static func failed(_ frame: OverlayFrame) -> Pose {
        let t = frame.phaseElapsed
        let shake = 4.0 * exp(-t * 7.0) * sin(2 * Double.pi * 7.5 * t)
        var pose = ending(frame, failC, .alert)
        pose.shift = shake
        return pose
    }

    static func poses(_ frame: OverlayFrame) -> Pose {
        switch frame.phase {
        case .recording: return recording(frame)
        case .transcribing: return transcribing(frame)
        case .polishing: return polishing(frame)
        case .done: return done(frame)
        case .cancelled: return cancelled(frame)
        case .failed: return failed(frame)
        }
    }

    /// The premultiplied planar colour `c`, alpha `a` and capsule distance `d` over the computed box, row-major.
    struct Canvas {
        let c0, c1, c2, a, d: UnsafeMutablePointer<Float>
        let cols: Int
    }

    /// Composite straight `rgb` at `alpha` over the premultiplied canvas pixel `i`.
    @inline(__always) static func over(_ canvas: Canvas, _ i: Int, _ rgb: SIMD3<Float>, _ alpha: Float) {
        let keep = 1 - alpha
        canvas.c0[i] = canvas.c0[i] * keep + rgb.x * alpha
        canvas.c1[i] = canvas.c1[i] * keep + rgb.y * alpha
        canvas.c2[i] = canvas.c2[i] * keep + rgb.z * alpha
        canvas.a[i] = canvas.a[i] * keep + alpha
    }

    /// Distance from (px, py) to the segment from (ax, ay) to (bx, by).
    struct Segment {
        let ax, ay, dx, dy, length2: Float

        init(_ ax: Double, _ ay: Double, _ bx: Double, _ by: Double) {
            let dx = bx - ax, dy = by - ay
            self.ax = Float(ax)
            self.ay = Float(ay)
            self.dx = Float(dx)
            self.dy = Float(dy)
            length2 = Float(max(dx * dx + dy * dy, 1e-6))
        }

        @inline(__always) func distance(_ px: Float, _ py: Float) -> Float {
            let h = clip(((px - ax) * dx + (py - ay) * dy) / length2)
            return hypotf(px - ax - dx * h, py - ay - dy * h)
        }
    }

    /// Signed distance in device pixels, and colour, of a glyph centred on (0, 0).
    struct GlyphShape {
        let kind: Glyph
        let rgb: SIMD3<Float>
        // latch
        var bodyX: Float = 0, bodyY: Float = 0, bodyH: Float = 0, bodyR: Float = 0
        var arcY: Float = 0, arcR: Float = 0, shackleEnd: Float = 0, shackleR: Float = 0
        var leg = Segment(0, 0, 0, 0)
        // check, and the alert's stem
        var first = Segment(0, 0, 0, 0), second: Segment? = nil, strokeR: Float = 0
        // alert
        var dotY: Float = 0, dotR: Float = 0

        init(_ kind: Glyph, _ t: Double, _ s: Double) {
            self.kind = kind
            switch kind {
            case .latch:
                rgb = latchC
                bodyX = Float(3.3 * s)
                bodyY = Float(2.0 * s)
                bodyH = Float(2.3 * s)
                bodyR = Float(1.0 * s)
                arcY = Float(2.0 * s)
                arcR = Float(2.5 * s)
                leg = Segment(2.5 * s, -2.0 * s, 2.5 * s, -0.5 * s)
                shackleEnd = Float(-2.0 * s)
                shackleR = Float(0.75 * s)
            case .check:
                rgb = doneC
                let drawn = ease((t - 0.04) / 0.22)
                let (ax, ay, bx, by, cx, cy) = (-7.0 * s, 0.4 * s, -2.3 * s, 5.0 * s, 7.6 * s, -5.8 * s)
                let first = 0.36
                var u = min(1.0, drawn / first)
                self.first = Segment(ax, ay, ax + (bx - ax) * u, ay + (by - ay) * u)
                if drawn > first {
                    u = (drawn - first) / (1 - first)
                    second = Segment(bx, by, bx + (cx - bx) * u, by + (cy - by) * u)
                }
                strokeR = Float(1.6 * s)
            case .alert:
                rgb = failC
                first = Segment(0, -7.0 * s, 0, 1.6 * s)
                strokeR = Float(1.6 * s)
                dotY = Float(6.2 * s)
                dotR = Float(1.75 * s)
            }
        }

        @inline(__always) func distance(_ px: Float, _ py: Float) -> Float {
            switch kind {
            case .latch:
                let qx = abs(px) - bodyX, qy = abs(py - bodyY) - bodyH
                let body = hypotf(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - bodyR
                let arc = abs(hypotf(px, py + arcY) - arcR)
                let legD = leg.distance(abs(px), py)
                let shackle = (py < shackleEnd ? arc : legD) - shackleR
                return min(body, shackle)
            case .check:
                var d = first.distance(px, py)
                if let second {
                    d = min(d, second.distance(px, py))
                }
                return d - strokeR
            case .alert:
                let stem = first.distance(px, py) - strokeR
                let dot = hypotf(px, py - dotY) - dotR
                return min(stem, dot)
            }
        }
    }

    /// Premultiplied colour, alpha and signed distance of the shadowed glass capsule at these columns.
    static func shell(
        _ canvas: Canvas, _ qx2: [Float], _ dy: [Float], _ halfH: Double, _ s: Double, _ rimRGB: SIMD3<Float>,
        _ rimStrength: Double, flat m0: Int, _ flatN: Int
    ) {
        let halfHF = Float(halfH)
        let shadowDrop = Float(4 * s)
        let soft = Float(2 * s), softWidth = Float(13 * s), hardWidth = Float(3 * s)
        let sF = Float(s), halfS = Float(0.5 * s), rimWidth = Float(0.75 * s)
        let depthSpan = Float(2 * halfH)
        let rimBoost = Float(0.55 * rimStrength)
        let cols = canvas.cols
        qx2.withUnsafeBufferPointer { qx2 in
            for r in 0..<dy.count {
                let dy = dy[r]
                let dy2 = sq(dy)
                let shadowDy2 = sq(dy - shadowDrop)
                let depth = clip((dy + halfHF) / depthSpan)
                let glass = glassTop + (glassBottom - glassTop) * depth
                // A light hairline just inside the edge holds the shape on dark backgrounds, a dark one just
                // outside holds it on white; the top of the light one doubles as the glass highlight.
                let top = powf(clip(-dy / halfHF), 1.5)
                let innerLight = 0.1 + 0.22 * top + rimBoost
                let row = r * cols

                @inline(__always) func pixel(_ j: Int) {
                    let q = qx2[j], i = row + j
                    let d = (q + dy2).squareRoot() - halfHF
                    let shadowD = (q + shadowDy2).squareRoot() - halfHF
                    let shadow = max(
                        0.34 * sq(clip(1 - (shadowD + soft) / softWidth)),
                        0.22 * clip(1 - (shadowD - soft) / hardWidth))
                    let cover = clip(0.5 - d) * glassAlpha
                    canvas.c0[i] = glass.x * cover
                    canvas.c1[i] = glass.y * cover
                    canvas.c2[i] = glass.z * cover
                    canvas.a[i] = cover + shadow * (1 - cover)
                    canvas.d[i] = d
                    let inner = clip(1 - abs(d + sF) / rimWidth) * innerLight
                    let outer = clip(1 - abs(d - halfS) / rimWidth) * 0.35
                    over(canvas, i, rimRGB * (inner / max(inner + outer, 1e-6)), max(inner, outer))
                }

                if flatN == 0 {
                    for j in 0..<cols { pixel(j) }
                    continue
                }
                for j in 0...m0 { pixel(j) }
                let m = row + m0
                let (c0, c1, c2, a, d) = (canvas.c0[m], canvas.c1[m], canvas.c2[m], canvas.a[m], canvas.d[m])
                for j in m0 + 1..<m0 + flatN {
                    canvas.c0[row + j] = c0
                    canvas.c1[row + j] = c1
                    canvas.c2[row + j] = c2
                    canvas.a[row + j] = a
                    canvas.d[row + j] = d
                }
                for j in m0 + flatN..<cols { pixel(j) }
            }
        }
    }

    /// Draw the mirrored bars, their bloom and the spectral aura into the premultiplied `c` and `a`.
    static func bars(
        _ pose: Pose, _ canvas: Canvas, rows: Range<Int>, cols: Range<Int>, _ x: [Float], _ y: [Float],
        _ cx: Double, _ cy: Double, _ pitch: Double, _ s: Double
    ) {
        if rows.isEmpty || cols.isEmpty { return }
        let pitchF = Float(pitch)
        let centers = (0..<nBars).map { Float(cx) + (Float($0) - center) * pitchF }
        // The aura reaches zero at the box edge so it never shows a seam.
        let margin = Float(glowMargin * s)
        let auraAt = ([centers[0] - margin] + centers + [centers[nBars - 1] + margin]).map(Double.init)
        let auraAmp = ([0] + pose.aura + [0]).map(Double.init)
        let sF = Float(s)

        // Per column: the nearest bar and what it asks for.
        let n = cols.count
        var columns = [Float](repeating: 0, count: 8 * n)
        columns.withUnsafeMutableBufferPointer { buf in
            let dx = buf.baseAddress!, reachFrom = dx + n, barAlpha = dx + 2 * n
            let r0 = dx + 3 * n, g0 = dx + 4 * n, b0 = dx + 5 * n
            let auraWidth = dx + 6 * n, auraPeak = dx + 7 * n
            for (j, col) in cols.enumerated() {
                let x = x[col]
                let idx = Int(min(max(((x - centers[0]) / pitchF).rounded(.toNearestOrEven), 0), Float(nBars - 1)))
                dx[j] = x - centers[idx]
                reachFrom[j] = pose.halves[idx] * sF
                barAlpha[j] = pose.alpha[idx]
                r0[j] = pose.colors[idx].x
                g0[j] = pose.colors[idx].y
                b0[j] = pose.colors[idx].z
                let amp = Float(interp(Double(x), auraAt, auraAmp))
                auraWidth[j] = (5 + 12 * amp) * sF
                auraPeak[j] = amp * 0.32
            }

            let interiorFrom = Float(-1.5 * s)
            let barRadius = Float(barR * s)
            let bloomWidth = Float(1.8 * s)
            let alongSpan = Float((lMax + barR) * s)
            let flare = Float(pose.flare)
            for row in rows {
                let dy = abs(y[row] - Float(cy))
                // Brighter toward the tips, and loud tips flare toward white.
                let along = clip(dy / alongSpan)
                let white = flare * powf(along, 2.5)
                let gain = (0.7 + 0.3 * along) * (1 - white)
                let base = row * canvas.cols
                for (j, col) in cols.enumerated() {
                    let i = base + col
                    let interior = clip(interiorFrom - canvas.d[i])
                    let reach = max(dy - reachFrom[j], 0)
                    let barD = (sq(dx[j]) + sq(reach)).squareRoot() - barRadius
                    let barA = clip(0.5 - barD) * barAlpha[j] * interior
                    let keep = 1 - barA
                    let bloom = expf(-max(barD, 0) / bloomWidth) * keep * interior * 0.3
                    let aura = expf(-sq(dy / auraWidth[j])) * auraPeak[j] * interior
                    let lit = barA + bloom
                    let light = gain * lit + aura
                    let flash = white * lit
                    canvas.c0[i] = canvas.c0[i] * keep + r0[j] * light + flash
                    canvas.c1[i] = canvas.c1[i] * keep + g0[j] * light + flash
                    canvas.c2[i] = canvas.c2[i] * keep + b0[j] * light + flash
                    canvas.a[i] = canvas.a[i] * keep + barA
                }
            }
        }
    }

    public static func render(_ frame: OverlayFrame) -> RGBAImage {
        let w = frame.width, h = frame.height
        var image = RGBAImage(width: w, height: h)
        if frame.presence <= 0 {
            return image
        }
        let s = frame.scale
        let pose = poses(frame)

        // A dot fades in, then stretches into the capsule; leaving plays it backwards.
        let p = frame.presence
        let fullW = (capsuleW + (frame.handsFree ? 2 * latchW : 0)) * pose.width
        let dot = capsuleH * (0.5 + 0.5 * ease(p / 0.6))
        let halfH = dot * s / 2
        let halfW = (dot + (fullW - dot) * ease((p - 0.15) / 0.85)) * s / 2
        let cx = Double(w) / 2 + pose.shift * s
        let cy = Self.cy * s

        let x0 = max(0, Int(cx - halfW - 12 * s)), x1 = min(w, Int((cx + halfW + 12 * s).rounded(.up)))
        let y0 = max(0, Int(cy - halfH - 8 * s)), y1 = min(h, Int((cy + halfH + 16 * s).rounded(.up)))
        if x1 <= x0 || y1 <= y0 {
            return image
        }
        let x = (x0..<x1).map { Float($0) + 0.5 }
        let y = (y0..<y1).map { Float($0) + 0.5 }
        let cols = x.count, rows = y.count

        // Between the end caps every column is identical, so the shell is computed for the caps and
        // one middle column, and that column is repeated across the middle.
        let flat = halfW - halfH
        let qx2 = x.map { sq(max(abs($0 - Float(cx)) - Float(flat), 0)) }
        // np.searchsorted compares in float64.
        let m0 = x.firstIndex { Double($0) >= cx - flat } ?? cols
        let flatN = max(0, (x.firstIndex { Double($0) > cx + flat } ?? cols) - m0)
        let rimRGB = pose.rim ?? SIMD3<Float>(repeating: 1)

        func box(_ bx: Double, _ by: Double, _ rx: Double, _ ry: Double) -> (Range<Int>, Range<Int>) {
            let r0 = max(0, Int(by - ry) - y0), r1 = min(rows, max(0, Int((by + ry).rounded(.up)) - y0))
            let c0 = max(0, Int(bx - rx) - x0), c1 = min(cols, max(0, Int((bx + rx).rounded(.up)) - x0))
            return (min(r0, r1)..<r1, min(c0, c1)..<c1)
        }

        let n = rows * cols
        var planes = [Float](repeating: 0, count: 5 * n)
        planes.withUnsafeMutableBufferPointer { buf in
            let base = buf.baseAddress!
            let canvas = Canvas(c0: base, c1: base + n, c2: base + 2 * n, a: base + 3 * n, d: base + 4 * n, cols: cols)
            shell(canvas, qx2, y.map { $0 - Float(cy) }, halfH, s, rimRGB, pose.rimStrength, flat: m0, flatN)

            let pitch = Self.pitch * pose.pitch * s
            let (barRows, barCols) = box(cx, cy, Double(center) * pitch + glowMargin * s, (lMax + barR + 6.5) * s)
            bars(pose, canvas, rows: barRows, cols: barCols, x, y, cx, cy, pitch, s)

            if let kind = pose.glyph {
                let gx = kind == .latch ? cx - halfW + 17.5 * s : cx
                let (glyphRows, glyphCols) = box(gx, cy, 14 * s, 14 * s)
                let glyph = GlyphShape(kind, pose.glyphT, s)
                let insideFrom = Float(-1.5 * s), glowWidth = Float(2.5 * s)
                let rgb = glyph.rgb
                for row in glyphRows {
                    let py = y[row] - Float(cy)
                    for col in glyphCols {
                        let i = row * cols + col
                        let gd = glyph.distance(x[col] - Float(gx), py)
                        let inside = clip(insideFrom - canvas.d[i])
                        let gA = clip(0.5 - gd) * inside
                        over(canvas, i, rgb, gA)
                        if kind != .latch {
                            let glow = expf(-max(gd, 0) / glowWidth) * (1 - gA) * inside * 0.35
                            canvas.c0[i] += rgb.x * glow
                            canvas.c1[i] += rgb.y * glow
                            canvas.c2[i] += rgb.z * glow
                        }
                    }
                }
            }

            // Additive light can push premultiplied colour past alpha; clamp it. The bytes stay premultiplied.
            let gain = Float(255 * min(1.0, p / 0.3))
            image.pixels.withUnsafeMutableBufferPointer { out in
                for row in 0..<rows {
                    var o = ((y0 + row) * w + x0) * 4
                    for i in row * cols..<(row + 1) * cols {
                        let a = canvas.a[i]
                        out[o] = byte(min(canvas.c0[i], a) * gain + 0.5)
                        out[o + 1] = byte(min(canvas.c1[i], a) * gain + 0.5)
                        out[o + 2] = byte(min(canvas.c2[i], a) * gain + 0.5)
                        out[o + 3] = byte(a * gain + 0.5)
                        o += 4
                    }
                }
            }
        }
        return image
    }

    // MARK: - NumPy semantics

    @inline(__always) static func sq(_ v: Float) -> Float { v * v }

    @inline(__always) static func clip(_ v: Float) -> Float { min(max(v, 0), 1) }

    /// Truncates like NumPy's float to uint8 cast, which the Python relies on after adding 0.5.
    @inline(__always) static func byte(_ v: Float) -> UInt8 { v >= 255 ? 255 : v > 0 ? UInt8(v) : 0 }

    /// Python's and NumPy's `%`: the result takes the sign of the divisor.
    static func pyMod(_ a: Double, _ b: Double) -> Double {
        var m = fmod(a, b)
        if m != 0, (b < 0) != (m < 0) { m += b }
        return m
    }

    static func pyMod(_ a: Float, _ b: Float) -> Float {
        var m = fmodf(a, b)
        if m != 0, (b < 0) != (m < 0) { m += b }
        return m
    }

    /// np.interp at one point: float64 arithmetic, held flat beyond the ends.
    static func interp(_ x: Double, _ xp: [Double], _ fp: [Double]) -> Double {
        let last = xp.count - 1
        if x < xp[0] { return fp[0] }
        if x >= xp[last] { return fp[last] }
        var j = 0, hi = last
        while hi - j > 1 {
            let mid = (j + hi) / 2
            if xp[mid] <= x { j = mid } else { hi = mid }
        }
        if xp[j] == x { return fp[j] }
        let slope = (fp[j + 1] - fp[j]) / (xp[j + 1] - xp[j])
        return slope * (x - xp[j]) + fp[j]
    }
}
