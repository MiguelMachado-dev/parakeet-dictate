// A port of src/parakeet_dictate/styles/fluid.py. NumPy's float32 arrays are Float here, its Python float scalars
// Double, cast to Float where they meet an array, in the same order of operations.

import Foundation

/// Plumage: voice ribbons in budgerigar colours, held in a night-blue capsule.
public enum Fluid {
    // Logical pixels, multiplied by frame.scale.
    static let halfLen = 134.0
    static let radius = 26.0
    static let lift = 3.0
    static let shadowDrop = 4.0
    static let shadowReach = 15.0
    static let shadowAlpha = 0.45
    static let haloReach = 13.0
    static let haloAlpha = 0.55
    static let crestRoom = 5.0
    static let capReach = 0.55
    static let lockRoom = 16.0
    // How long the hands-free lock lingers into each phase that can directly follow a latched recording.
    static let lockFadeS: [Phase: Double] = [.transcribing: 0.08, .cancelled: 0.16]
    static let glowReach = 7.0

    static let membrane: Float = 0.3, edge: Float = 1.0, glow: Float = 0.3

    // History hops a ripple takes to travel from the middle of the capsule to its ends.
    static let rippleHops = 34.0
    static let rippleCore = 0.14
    static let smooth: [Float] = [1, 2, 3, 2, 1].map { Float($0) / 9 }

    static func rgb(_ hexCode: String) -> [Float] {
        let value = Int(hexCode.dropFirst(), radix: 16)!
        return [16, 8, 0].map { Float(Double((value >> $0) & 0xff) / 255) }
    }

    /// (2, 3): top and bottom.
    static func body(_ top: String, _ bottom: String) -> [Float] { rgb(top) + rgb(bottom) }

    static let night = body("#1a2234", "#090e18")
    static let indigo = body("#251a4a", "#0d0920")
    static let moss = body("#173a26", "#08170f")
    static let wine = body("#3a1820", "#170a0d")
    static let graphite = body("#20232a", "#0d0f13")
    static let rim = rgb("#a9b8d6")
    static let white = rgb("#ffffff")
    static let cobalt = rgb("#3a6bff"), sky = rgb("#1fc8f0"), green = rgb("#79ec4f"), lemon = rgb("#ffd84a")
    static let violet = rgb("#8f63ff"), orchid = rgb("#e46cff")
    static let coral = rgb("#ff6a5c"), ash = rgb("#9aa3b2")
    static let lockInk = rgb("#dfe6f5"), sparkleInk = rgb("#f1e9ff"), checkInk = rgb("#b6ff8f")
    /// (4, 3)
    static let plumage = cobalt + sky + green + lemon
    static let thinking = violet + sky + orchid + cobalt

    // Per ribbon: spatial cycles across the span, phase speed (rad/s), phase offset, twist cycles.
    static let cycles: [Float] = [1.25, 1.7, 2.2, 2.85]
    static let speed: [Float] = [2.1, -2.7, 3.3, -4.1]
    static let offset: [Float] = [0.0, 2.1, 4.0, 5.5]
    static let twist: [Float] = [0.9, 1.3, 1.1, 1.6]
    static let quarters: [Float] = (0..<4).map { Float($0) * Float(Double.pi / 2) }

    /// np.pi meeting a float32 array. (Float.pi is rounded towards zero, so it is not the same number.)
    static let pi = Float(Double.pi)

    /// Four ribbons sampled at each column of the capsule, and the light and body around them.
    struct Look {
        var center: [Float]  // (4, X) crest offset from the midline, px
        var half: [Float]  // (4, X) half-width of the ribbon membrane, px
        var gain: [Float]  // (4, X) brightness
        var color: [Float]  // (4, 3)
        var aura: [Float]  // (3, X) soft light washed through the capsule behind the ribbons
        var body: [Float]  // (2, 3) top and bottom of the capsule

        func mix(_ other: Look, _ m: Double) -> Look {
            if m <= 0 {
                return self
            }
            if m >= 1 {
                return other
            }
            let m = Float(m)
            return Look(
                center: lerp(center, other.center, m), half: lerp(half, other.half, m), gain: lerp(gain, other.gain, m),
                color: lerp(color, other.color, m), aura: lerp(aura, other.aura, m), body: lerp(body, other.body, m))
        }
    }

    struct Stage {
        let frame: OverlayFrame
        let u: [Float]  // (X,) column position along the ribbon span, -1..1
        let taper: [Float]  // (X,) 1 mid-span, 0 at the span ends
        let reach: Double  // px a crest may travel from the midline
        let s: Double

        var t: Double { frame.phaseElapsed }
    }

    /// a + (b - a) * m, elementwise.
    static func lerp(_ a: [Float], _ b: [Float], _ m: Float) -> [Float] {
        var out = a
        for i in out.indices {
            out[i] = a[i] + (b[i] - a[i]) * m
        }
        return out
    }

    static func smoothstep(_ e0: Double, _ e1: Double, _ x: Double) -> Double {
        let x = min(max((x - e0) / (e1 - e0), 0.0), 1.0)
        return x * x * (3.0 - 2.0 * x)
    }

    static func clip(_ x: Float) -> Float { min(max(x, 0), 1) }

    /// Python's and NumPy's %: the remainder takes the divisor's sign.
    static func mod<T: FloatingPoint>(_ a: T, _ b: T) -> T {
        var m = a.truncatingRemainder(dividingBy: b)
        if m != 0 && (b < 0) != (m < 0) {
            m += b
        }
        return m
    }

    /// NumPy's mean of float32: a float32 sum, divided in float64.
    static func mean(_ values: ArraySlice<Float>) -> Float {
        Float(Double(pairwiseSum(values)) / Double(values.count))
    }

    /// NumPy's float32 add.reduce: pairwise, eight accumulators per block.
    static func pairwiseSum(_ a: ArraySlice<Float>) -> Float {
        let n = a.count, i0 = a.startIndex
        if n < 8 {
            var res: Float = 0
            for v in a {
                res += v
            }
            return res
        }
        if n <= 128 {
            var r = (0..<8).map { a[i0 + $0] }
            var i = 8
            while i < n - n % 8 {
                for j in 0..<8 {
                    r[j] += a[i0 + i + j]
                }
                i += 8
            }
            var res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]))
            while i < n {
                res += a[i0 + i]
                i += 1
            }
            return res
        }
        var n2 = n / 2
        n2 -= n2 % 8
        return pairwiseSum(a[i0..<(i0 + n2)]) + pairwiseSum(a[(i0 + n2)...])
    }

    /// The level history laid out from the middle (now) to the span ends (RIPPLE_HOPS ago).
    static func ripple(_ levels: [Float], _ u: [Float]) -> [Float] {
        let levels = levels.isEmpty ? [0] : levels
        let padded = [levels[0], levels[0]] + levels + [levels[levels.count - 1], levels[levels.count - 1]]
        let kernel = Self.smooth
        var smooth = [Float](repeating: 0, count: padded.count - kernel.count + 1)
        // np.convolve(padded, SMOOTH, mode="valid"), fused like Accelerate's.
        for i in smooth.indices {
            var sum: Float = 0
            for k in kernel.indices {
                sum = sum.addingProduct(padded[i + k], kernel[kernel.count - 1 - k])
            }
            smooth[i] = sum
        }
        // Hyperbolic rather than |u|, so a fresh onset swells as a rounded crest instead of a spike.
        // The denominator is a float64 NumPy scalar, so away, age and the interpolation are float64.
        let core = Float(rippleCore), core2 = Float(rippleCore * rippleCore)
        let full = (1 + rippleCore * rippleCore).squareRoot() - rippleCore
        let last = smooth.count - 1
        return u.map { u in
            let away = Double((u * u + core2).squareRoot() - core) / full
            let age = Double(last) - away * rippleHops
            // np.interp(age, np.arange(len(smooth)), smooth)
            if age <= 0 {
                return smooth[0]
            }
            if age >= Double(last) {
                return smooth[last]
            }
            let j = Int(age)
            let slope = Double(smooth[j + 1]) - Double(smooth[j])
            return Float(slope * (age - Double(j)) + Double(smooth[j]))
        }
    }

    static func voice(_ st: Stage) -> Look {
        let f = st.frame
        let x = st.u.count
        let bands = max(f.spectrum.count / 4, 1)
        let spectrum = f.spectrum.count >= 4 ? f.spectrum : [Float](repeating: 0, count: 4)
        let energy = (0..<4).map { mean(spectrum[($0 * bands)..<(($0 + 1) * bands)]) }
        let share = energy.map { $0 / Float(max(Double(energy.max()!), 0.05)) }
        let total = Float(Double(energy[0] + energy[1] + energy[2] + energy[3]) + 0.04)
        let tint = (0..<3).map { c in
            // energy @ PLUMAGE, fused like Accelerate's.
            var dot = energy[0] * plumage[c]
            for k in 1..<4 {
                dot = dot.addingProduct(energy[k], plumage[k * 3 + c])
            }
            return (dot + 0.04 * sky[c]) / total
        }
        let ripple = ripple(f.levels, st.u)
        let t = f.recordingElapsed
        let breathe = (0..<4).map { 0.85 + 0.15 * sinf(Float(2 * Double.pi * t / 2.6) + offset[$0]) }
        let reach = Float(st.reach), s = Float(st.s)
        let waveDrift = Float(1.7 * t), twistDrift = Float(1.3 * t)
        var center = [Float](repeating: 0, count: 4 * x), half = center, gain = center
        for k in 0..<4 {
            let cycles = pi * Self.cycles[k], speed = Self.speed[k] * Float(t), twist = pi * Self.twist[k]
            let offset = Self.offset[k], breathe = breathe[k], share = 0.55 + 0.45 * share[k]
            for i in 0..<x {
                let u = st.u[i], taper = st.taper[i]
                let drive = ripple[i] * share
                let phase = cycles * u + speed + offset
                let wave = 0.8 * sinf(phase) + 0.2 * sinf(0.55 * phase - waveDrift)
                let amp = reach * taper * min(0.3 * breathe + 1.1 * drive, 1.0)
                let twisted = abs(cosf(twist * u - twistDrift + offset))
                center[k * x + i] = amp * wave
                half[k * x + i] = s * (0.5 + (0.7 + 5.0 * drive) * twisted) * powf(taper, 0.7)
                gain[k * x + i] = (0.6 + 1.1 * drive) * breathe * powf(taper, 0.6)
            }
        }
        let loudness = Float(0.05 + 0.4 * Double(mean(f.levels.suffix(3))))
        var aura = [Float](repeating: 0, count: 3 * x)
        for i in 0..<x {
            let wash = loudness * powf(st.taper[i], 1.5)
            for c in 0..<3 {
                aura[c * x + i] = tint[c] * wash
            }
        }
        return Look(center: center, half: half, gain: gain, color: plumage, aura: aura, body: night)
    }

    static func rope(_ st: Stage) -> Look {
        let x = st.u.count, t = st.t
        let sweep = Float(-1.15 + 2.55 * mod(t / 0.5, 1.0))
        let shine = st.u.map { u in
            let q = (u - sweep) / 0.16
            return expf(-(q * q))
        }
        let cycles = Float(Double.pi * 3.0), drift = Float(16.0 * t)
        let crest = Float(0.16 * st.reach), s = Float(st.s)
        var center = [Float](repeating: 0, count: 4 * x), half = center, gain = center
        for k in 0..<4 {
            for i in 0..<x {
                let taper = st.taper[i]
                let phase = cycles * st.u[i] - drift + quarters[k]
                center[k * x + i] = crest * powf(taper, 1.2) * sinf(phase)
                half[k * x + i] = s * (0.3 + 0.6 * abs(cosf(phase))) * powf(taper, 0.7)
                gain[k * x + i] = (0.6 + 2.2 * shine[i]) * powf(taper, 0.6)
            }
        }
        let tint = (0..<3).map { sky[$0] + (white[$0] - sky[$0]) * 0.5 }
        var aura = [Float](repeating: 0, count: 3 * x)
        for i in 0..<x {
            let wash = (0.06 + 0.3 * shine[i]) * st.taper[i]
            for c in 0..<3 {
                aura[c * x + i] = tint[c] * wash
            }
        }
        let color = plumage.indices.map { plumage[$0] + (white[$0 % 3] - plumage[$0]) * 0.5 }
        return Look(center: center, half: half, gain: gain, color: color, aura: aura, body: night)
    }

    static func transcribing(_ st: Stage) -> Look {
        voice(st).mix(rope(st), smoothstep(0.0, 0.08, st.t))
    }

    static func polishing(_ st: Stage) -> Look {
        let x = st.u.count, t = st.t
        let sign: [Float] = [1, -1, 1, -1]
        let lag = ([0, 0, 1, 1] as [Float]).map { $0 * Float(Double.pi / 2) }
        let size: [Float] = [1, 1, 0.55, 0.55]
        let breath = sin(2 * Double.pi * t / 1.9)
        let cycles = Float(Double.pi * 1.6), drift = Float(2.4 * t)
        let swell = Float(0.5 + 0.16 * breath), reach = Float(st.reach), s = Float(st.s)
        let shimmerCycles = Float(1.2 * Double.pi), shimmerDrift = Float(1.7 * t)
        let sparkAt = quarters.map { -1.3 + 2.6 * mod(Float(t / 1.7) + $0 * 0.17, 1.0) }
        var center = [Float](repeating: 0, count: 4 * x), half = center, gain = center
        for k in 0..<4 {
            let crest = sign[k] * size[k] * swell * reach
            for i in 0..<x {
                let u = st.u[i], taper = st.taper[i]
                let phase = cycles * u - drift + lag[k]
                center[k * x + i] = crest * powf(taper, 1.3) * sinf(phase)
                half[k * x + i] = s * (0.5 + 3.2 * abs(cosf(phase))) * powf(taper, 0.7)
                let shimmer = 0.5 + 0.5 * cosf(shimmerCycles * u - shimmerDrift + quarters[k])
                let q = (u - sparkAt[k]) / 0.05
                let spark = expf(-(q * q))
                gain[k * x + i] = (shimmer + 1.6 * spark) * powf(taper, 0.6)
            }
        }
        let glow = Float(0.14 + 0.06 * breath)
        var aura = [Float](repeating: 0, count: 3 * x)
        for i in 0..<x {
            let wash = glow * powf(st.taper[i], 1.5)
            for c in 0..<3 {
                aura[c * x + i] = violet[c] * wash
            }
        }
        let helix = Look(center: center, half: half, gain: gain, color: thinking, aura: aura, body: indigo)
        return rope(st).mix(helix, smoothstep(0.0, 0.3, t))
    }

    static func line(
        _ st: Stage, _ color: [Float], _ body: [Float], _ span: Double, _ flash: Double, droop: Double = 0.0
    ) -> Look {
        let x = st.u.count
        let width = Float(max(span, 1e-3)), sag = Float(droop * st.reach), thick = Float(st.s * 0.9)
        let bright = Float(flash), wash = Float(0.12 * flash)
        var center = [Float](repeating: 0, count: 4 * x), half = center, gain = center
        var aura = [Float](repeating: 0, count: 3 * x)
        for i in 0..<x {
            let u = st.u[i]
            let kept = clip(1.0 - abs(u) / width)
            let taper = min(st.taper[i], kept * 3.0)
            let dip = sag * (1.0 - u * u) * st.taper[i]
            let thickness = thick * powf(taper, 0.7)
            let shine = bright * powf(taper, 0.6)
            for k in 0..<4 {
                center[k * x + i] = dip
                half[k * x + i] = thickness
                gain[k * x + i] = shine
            }
            for c in 0..<3 {
                aura[c * x + i] = color[c] * (wash * st.taper[i])
            }
        }
        let colors = color + color + color + color
        return Look(center: center, half: half, gain: gain, color: colors, aura: aura, body: body)
    }

    static func done(_ st: Stage) -> Look {
        let t = min(st.t, OverlayCanvas.beat)
        let flash = 0.9 + 1.4 * exp(-(((t - 0.08) / 0.07) * ((t - 0.08) / 0.07)))
        let mint = (0..<3).map { green[$0] + (white[$0] - green[$0]) * 0.25 }
        let line = line(st, mint, moss, 1.0 - smoothstep(0.04, 0.17, t), flash)
        return rope(st).mix(line, smoothstep(0.0, 0.08, t))
    }

    static func cancelled(_ st: Stage) -> Look {
        let t = min(st.t, OverlayCanvas.beat)
        let line = line(st, ash, graphite, 1.0 - smoothstep(0.12, 0.42, t), 0.7)
        return voice(st).mix(line, smoothstep(0.0, 0.16, t))
    }

    static func failed(_ st: Stage) -> Look {
        let t = min(st.t, OverlayCanvas.beat)
        let line = line(st, coral, wine, 1.0, 1.1, droop: 0.3 * smoothstep(0.05, 0.3, t))
        return rope(st).mix(line, smoothstep(0.0, 0.1, t))
    }

    static func looks(_ phase: Phase) -> (Stage) -> Look {
        switch phase {
        case .recording: return voice
        case .transcribing: return transcribing
        case .polishing: return polishing
        case .done: return done
        case .cancelled: return cancelled
        case .failed: return failed
        }
    }

    /// The four ribbons of a look, ready to light one row at a time.
    struct Ribbons {
        let x: Int
        // (4, X) each: crest offset, half-width, gain, 1 / sqrt(1 + slope²); then the (4, 3) colours.
        let table: [Float]
        let falloff: Float  // -1 / (s * GLOW_REACH)

        init(_ look: Look, _ s: Double) {
            let c = look.center
            let x = c.count / 4
            var slope = [Float](repeating: 0, count: 4 * x)
            if x >= 3 {
                for k in 0..<4 {
                    let n = k * x
                    for i in 1..<(x - 1) {
                        slope[n + i] = (c[n + i + 1] - c[n + i - 1]) * 0.5
                    }
                    slope[n] = slope[n + 1]
                    slope[n + x - 1] = slope[n + x - 2]
                }
            }
            let squash = slope.map { 1.0 / (1.0 + $0 * $0).squareRoot() }
            self.x = x
            self.table = look.center + look.half + look.gain + squash + look.color
            self.falloff = Float(-1.0 / (s * glowReach))
        }

        /// Tone-mapped additive light (3, X) of the four ribbons on row y (an offset from the midline).
        func light(_ y: Float, into light: UnsafeMutablePointer<Float>) {
            let x = self.x, f = falloff
            table.withUnsafeBufferPointer { table in
                let c = table.baseAddress!, h = c + 4 * x, g = h + 4 * x, q = g + 4 * x, color = q + 4 * x
                for i in 0..<x {
                    let i1 = x + i, i2 = 2 * x + i, i3 = 3 * x + i
                    let acc0 = Ribbons.acc(y, center: c[i], squash: q[i], half: h[i], gain: g[i], falloff: f)
                    let acc1 = Ribbons.acc(y, center: c[i1], squash: q[i1], half: h[i1], gain: g[i1], falloff: f)
                    let acc2 = Ribbons.acc(y, center: c[i2], squash: q[i2], half: h[i2], gain: g[i2], falloff: f)
                    let acc3 = Ribbons.acc(y, center: c[i3], squash: q[i3], half: h[i3], gain: g[i3], falloff: f)
                    // np.tensordot(look.color.T, acc, axes=1), fused like Accelerate's.
                    for c in 0..<3 {
                        var l = color[c] * acc0
                        l = l.addingProduct(color[3 + c], acc1)
                        l = l.addingProduct(color[6 + c], acc2)
                        l = l.addingProduct(color[9 + c], acc3)
                        light[c * x + i] = l
                    }
                }
            }
            for i in 0..<(3 * x) {
                light[i] = 1.0 - expf(-light[i])
            }
        }

        /// One ribbon's membrane, edge and glow at one pixel, times its gain.
        @inline(__always)
        private static func acc(
            _ y: Float, center: Float, squash: Float, half: Float, gain: Float, falloff: Float
        ) -> Float {
            let edge = abs(y - center) * squash - half
            var acc = min(max(0.5 - edge, 0), 1) * membrane
            acc += max(1.0 - abs(edge), 0) * Fluid.edge
            var tmp = min(max(edge * falloff + 1.0, 0), 1)
            tmp *= tmp
            tmp *= tmp
            tmp *= glow
            acc += tmp
            return acc * gain
        }
    }

    static func segment(_ px: Float, _ py: Float, _ a: (Double, Double), _ b: (Double, Double)) -> Float {
        let bax = b.0 - a.0, bay = b.1 - a.1
        let pax = px - Float(a.0), pay = py - Float(a.1)
        let h = clip((pax * Float(bax) + pay * Float(bay)) / Float(max(bax * bax + bay * bay, 1e-9)))
        return hypotf(pax - Float(bax) * h, pay - Float(bay) * h)
    }

    static func lockSDF(_ px: Float, _ py: Float) -> Float {
        let qx = abs(px) - 3.3, qy = abs(py - 2.6) - 2.3
        let body = hypotf(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - 1.2
        let arc = abs(hypotf(px, py + 0.6) - 2.8) - 0.85
        let legs = max(abs(abs(px) - 2.8) - 0.85, py - 2.0)
        return min(body, py < -0.6 ? arc : legs)
    }

    static func sparkleSDF(_ px: Float, _ py: Float, _ size: Double, _ turn: Double) -> Float {
        let c = Float(cos(turn)), s = Float(sin(turn))
        let x = abs(c * px - s * py), y = abs(s * px + c * py)
        let thin = size * 0.3
        let norm = Float(size * thin / hypot(size, thin))
        let size = Float(size), thinF = Float(thin)
        return min((x / size + y / thinF - 1) * norm, (x / thinF + y / size - 1) * norm)
    }

    static let checkPoints = [(-7.6, 0.4), (-2.4, 5.6), (8.0, -5.6)]

    static func checkSDF(_ px: Float, _ py: Float, _ progress: Double) -> Float {
        let pts = checkPoints
        let lengths = (hypot(pts[1].0 - pts[0].0, pts[1].1 - pts[0].1), hypot(pts[2].0 - pts[1].0, pts[2].1 - pts[1].1))
        var left = progress * (lengths.0 + lengths.1)
        var d = Float.infinity
        for n in 0..<2 {
            let a = pts[n], b = pts[n + 1], length = n == 0 ? lengths.0 : lengths.1
            let k = min(left / length, 1.0)
            if k <= 0 {
                break
            }
            d = min(d, segment(px, py, a, (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)))
            left -= length
        }
        return d - 1.9
    }

    /// How present the hands-free lock is: fully while recording, fading out of the phases that follow it.
    static func latched(_ frame: OverlayFrame) -> Double {
        if !frame.handsFree {
            return 0.0
        }
        if frame.phase == .recording {
            return 1.0
        }
        guard let fade = lockFadeS[frame.phase] else {
            return 0.0
        }
        return 1.0 - smoothstep(0.0, fade, frame.phaseElapsed)
    }

    enum Shape {
        case lock
        case sparkle(size: Double, turn: Double)
        case check(progress: Double)

        func sdf(_ px: Float, _ py: Float) -> Float {
            switch self {
            case .lock: return lockSDF(px, py)
            case let .sparkle(size, turn): return sparkleSDF(px, py, size, turn)
            case let .check(progress): return checkSDF(px, py, progress)
            }
        }
    }

    enum Anchor {
        case left
        case center
    }

    typealias Glyph = (sdf: Shape, ink: [Float], anchor: Anchor, strength: Double)

    /// The phase's glyph: an SDF in logical px, its colour, where it sits, and its opacity.
    static func glyph(_ frame: OverlayFrame) -> Glyph? {
        let t = frame.phaseElapsed
        let latch = latched(frame)
        if latch > 0 {
            return (.lock, lockInk, .left, latch)
        }
        if frame.phase == .polishing {
            let size = 6.0 * (1.0 + 0.2 * sin(2 * Double.pi * t / 0.95)) * max(smoothstep(0.05, 0.35, t), 0.05)
            let turn = 0.35 * sin(2 * Double.pi * t / 3.1)
            return (.sparkle(size: size, turn: turn), sparkleInk, .left, 1.0)
        }
        if frame.phase == .done {
            let progress = smoothstep(0.14, 0.32, min(t, OverlayCanvas.beat))
            if progress > 0 {
                return (.check(progress: progress), checkInk, .center, 1.0)
            }
        }
        return nil
    }

    public static func render(_ frame: OverlayFrame) -> RGBAImage {
        let w = frame.width, h = frame.height
        var image = RGBAImage(width: w, height: h)
        if frame.presence <= 0 {
            return image
        }
        let s = frame.scale, p = frame.presence
        let radius = s * Self.radius * (0.4 + 0.6 * smoothstep(0.0, 0.5, p))
        let halfLen = s * Self.halfLen * pow(smoothstep(0.15, 1.0, p), 0.8)
        var shake = 0.0
        if frame.phase == .failed {
            let t = min(frame.phaseElapsed, OverlayCanvas.beat)
            shake = s * 5.0 * sin(2 * Double.pi * 11 * t) * exp(-t / 0.12)
        }
        let cx = Double(w) / 2 + shake, cy = Double(h) / 2 - s * lift

        let reach = s * max(shadowReach, haloReach)
        let ya = max(0, Int(cy - radius - reach) - 1), yb = min(h, Int(cy + radius + s * shadowDrop + reach) + 2)
        let xa = max(0, Int(cx - halfLen - radius - reach) - 1), xb = min(w, Int(cx + halfLen + radius + reach) + 2)
        let xs = (xa..<xb).map { Float($0) + 0.5 }
        let dy = (ya..<yb).map { Float($0) + 0.5 - Float(cy) }
        let dx2 = xs.map { x in
            let dx = max(abs(x - Float(cx)) - Float(halfLen), 0.0)
            return dx * dx
        }

        let ra = max(0, Int(cy - radius) - 1 - ya), rb = min(yb - ya, Int(cy + radius) + 2 - ya)
        let ca = max(0, Int(cx - halfLen - radius) - 1 - xa), cb = min(xb - xa, Int(cx + halfLen + radius) + 2 - xa)
        let boxXs = Array(xs[ca..<cb])

        let left = cx - halfLen - capReach * radius + s * lockRoom * latched(frame)
        let right = cx + halfLen + capReach * radius
        let mid = (left + right) / 2, span = max((right - left) / 2, 1.0)
        let u = boxXs.map { ($0 - Float(mid)) / Float(span) }
        let taper = u.map { clip(1.0 - $0 * $0) }
        let stage = Stage(frame: frame, u: u, taper: taper, reach: max(radius - s * crestRoom, 1.0), s: s)
        let look = looks(frame.phase)(stage)
        let content = smoothstep(0.05, 0.5, p)

        let cols = cb - ca
        let haloLight = (0..<3).map { mean(look.aura[($0 * cols)..<(($0 + 1) * cols)]) * Float(content) }
        let haloPeak = Double(haloLight.max()!)
        let halo = haloPeak > 1e-3
        let haloStrength = Float(haloAlpha * min(haloPeak * 3.0, 1.0))
        let haloTint = halo ? haloLight.map { $0 / Float(haloPeak) } : [0, 0, 0]

        // The body: the capsule gradient, its rim, the aura and the ribbons, one row of the box at a time.
        let top = Array(look.body[0..<3]), bottom = Array(look.body[3..<6])
        let auraLight = look.aura.map { -($0 * Float(content)) }
        let extent = Double(zip(look.center, look.half).map { abs($0) + $1 }.max()!) + s * (glowReach + 1.0)
        let ribbons = content > 0 ? Ribbons(look, s) : nil

        var glyphShape: Shape?, ink: [Float] = [0, 0, 0], gx = 0.0, glyphStrength: Float = 0
        if let (sdf, glyphInk, anchor, strength) = glyph(frame) {
            glyphShape = sdf
            ink = glyphInk
            gx = anchor == .left ? cx - halfLen : cx
            glyphStrength = Float(content * strength)
        }
        let near = Float(s * 12.0)
        let glyphCols = boxXs.indices.filter { abs(boxXs[$0] - Float(gx)) < near }

        let shadowFall = Float(1.0 / (s * shadowReach)), haloFall = Float(1.0 / (s * haloReach))
        let fade = Float(255.0 * smoothstep(0.0, 0.3, p))
        let radiusF = Float(radius), drop = Float(s * shadowDrop), shadowAlphaF = Float(shadowAlpha)
        let vScale = Float(2 * radius), qScale = Float(1.6 / radius), sF = Float(s)
        let tint0 = haloTint[0], tint1 = haloTint[1], tint2 = haloTint[2]
        let rimTone = Self.rim

        // dx2 and -aura * content for the whole box, then per row: d, the body's rgb (3, C), the ribbons' light (3, C).
        let width = xb - xa
        var scratch = dx2 + auraLight + [Float](repeating: 0, count: width + 6 * cols)
        scratch.withUnsafeMutableBufferPointer { scratch in
            image.pixels.withUnsafeMutableBufferPointer { pixels in
                let dx2 = scratch.baseAddress!, auraLight = dx2 + width, d = auraLight + 3 * cols
                let rgb = d + width, light = rgb + 3 * cols
                for row in 0..<(yb - ya) {
                    let y = dy[row]
                    let y2 = y * y
                    for i in 0..<width {
                        d[i] = (dx2[i] + y2).squareRoot() - radiusF
                    }
                    let inside = row >= ra && row < rb
                    if inside {
                        let v = clip((y + radiusF) / vScale)
                        let rimShade = 0.14 + 0.4 * ((1.0 - v) * (1.0 - v))
                        let q = y * qScale
                        let spread = 1.0 / (1.0 + q * q)
                        for c in 0..<3 {
                            let grad = top[c] + (bottom[c] - top[c]) * v, tone = rimTone[c]
                            for i in 0..<cols {
                                let rim = clip(1.0 - abs(d[ca + i] + 1.0)) * rimShade
                                var value = grad + (tone - grad) * rim
                                let aura = 1.0 - expf(auraLight[c * cols + i] * spread)
                                value += aura * (1.0 - value)
                                rgb[c * cols + i] = value
                            }
                        }
                        if let ribbons, abs(y) <= Float(extent) {
                            ribbons.light(y, into: light)
                            let content = Float(content)
                            for i in 0..<(3 * cols) {
                                rgb[i] += light[i] * content * (1.0 - rgb[i])
                            }
                        }
                        if let glyphShape, abs(y) < near, let c0 = glyphCols.first, let c1 = glyphCols.last {
                            for i in c0...c1 {
                                var g = 0.5 - glyphShape.sdf((boxXs[i] - Float(gx)) / sF, y / sF) * sF
                                g = clip(g) * glyphStrength
                                for c in 0..<3 {
                                    rgb[c * cols + i] += (ink[c] - rgb[c * cols + i]) * g
                                }
                            }
                        }
                        for i in 0..<(3 * cols) {
                            rgb[i] = clip(rgb[i])
                        }
                    }

                    // Premultiplied: the body over the halo spill, which takes the colour of the halo light.
                    let ys = y - drop
                    let ys2 = ys * ys
                    var out = ((ya + row) * w + xa) * 4
                    for i in 0..<width {
                        let d = d[i]
                        let dShadow = (dx2[i] + ys2).squareRoot() - radiusF
                        let cover = clip(0.5 - d)
                        let fall = clip(1.0 - dShadow * shadowFall)
                        var under = shadowAlphaF * fall * fall
                        var r: Float = 0, g: Float = 0, b: Float = 0
                        if halo {
                            let ring = clip(1.0 - d * haloFall)
                            let glow = haloStrength * ring * ring * ring
                            under = glow + under * (1.0 - glow)
                            let spill = glow * (1.0 - cover)
                            r = spill * tint0
                            g = spill * tint1
                            b = spill * tint2
                        }
                        let alpha = cover + under * (1.0 - cover)
                        if inside && i >= ca && i < cb {
                            r += rgb[i - ca] * cover
                            g += rgb[cols + i - ca] * cover
                            b += rgb[2 * cols + i - ca] * cover
                        }
                        let a8 = min(alpha * fade + 0.5, 255)
                        pixels[out] = UInt8(min(r * fade + 0.5, a8))
                        pixels[out + 1] = UInt8(min(g * fade + 0.5, a8))
                        pixels[out + 2] = UInt8(min(b * fade + 0.5, a8))
                        pixels[out + 3] = UInt8(a8)
                        out += 4
                    }
                }
            }
        }
        return image
    }
}
