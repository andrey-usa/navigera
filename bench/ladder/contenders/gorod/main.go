// Contender: go-rod (native Go CDP client, github.com/go-rod/rod).
//
// Same canonical session as the other contenders:
//   session — launch, goto A, title/count/extract evals, fill, click,
//             visible-count eval, screenshot, new tab -> goto B,
//             title/row-count evals, close
//   eval    — warm eval round-trip micro: goto A, then 200x eval title
//             (per-op latencies printed as JSON lines on stdout)
//   cold    — launch, new page, one eval, close (cold-start micro)
//
// Env: CHROME_BIN (chrome executable, required),
//      LADDER_BASE (fixture server base URL),
//      LADDER_EXTRACT_OUT, LADDER_SHOT_OUT (session mode),
//      LADDER_N_EVAL (eval mode, default 200).
//
// stdout: session -> one JSON line {mode, wall_ms, counts};
//         eval -> line 1 {mode, wall_ms}, line 2 {eval_ms: [...]};
//         cold -> one JSON line {mode, wall_ms}.
// The harness (ladder.py) measures this process with wait4 for exact
// CPU/RSS. On any error: message on stderr, non-zero exit.

package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/go-rod/rod"
	"github.com/go-rod/rod/lib/launcher"
	"github.com/go-rod/rod/lib/launcher/flags"
	"github.com/go-rod/rod/lib/proto"
)

// NOTE on invocation: rod's Page.Eval wraps the JS as
// `function() { return (JS).apply(this, arguments) }`, i.e. it invokes a
// bare function expression itself. An explicit IIFE would break here: its
// non-function result has no `.apply`. Bare arrows are therefore the
// correct, deterministically-invoked form for this driver (verified against
// rod v0.116.2 page_eval.go).
const (
	evalTitle     = `() => document.title`
	evalCardCount = `() => document.querySelectorAll('.card').length`
	evalExtract   = `() => Array.from(document.querySelectorAll('.card')).map(c => ({t: c.querySelector('.t').textContent, p: c.querySelector('.p').textContent, v: c.dataset.vendor}))`
	evalVisible   = `() => document.querySelectorAll('.card:not(.hidden)').length`
	evalRows      = `() => document.querySelectorAll('#rows tr').length`
	evalOne       = `() => 1 + 1`
)

// Card mirrors the extract shape the correctness gate compares.
type Card struct {
	T string `json:"t"`
	P string `json:"p"`
	V string `json:"v"`
}

// newBrowser launches Chrome with the ladder's flags and connects.
func newBrowser(chromeBin string) (*rod.Browser, error) {
	u, err := launcher.New().
		Bin(chromeBin).
		HeadlessNew(true).
		Set(flags.NoSandbox).
		Set(flags.Flag("disable-dev-shm-usage")).
		Set(flags.Flag("no-first-run")).
		Set(flags.Flag("window-size"), "1440,900").
		Launch()
	if err != nil {
		return nil, fmt.Errorf("launch: %w", err)
	}
	browser := rod.New().ControlURL(u)
	if err := browser.Connect(); err != nil {
		return nil, fmt.Errorf("connect: %w", err)
	}
	return browser, nil
}

func setViewport(page *rod.Page) error {
	return page.SetViewport(&proto.EmulationSetDeviceMetricsOverride{
		Width: 1440, Height: 900, DeviceScaleFactor: 1,
	})
}

// evalStr / evalInt evaluate js and decode the by-value result.
// (Page.Eval returns (*proto.RuntimeRemoteObject, error); the decoded value
// lives in res.Value, a gson.JSON.)
func evalStr(page *rod.Page, js string) (string, error) {
	res, err := page.Eval(js)
	if err != nil {
		return "", err
	}
	return res.Value.Str(), nil
}

func evalInt(page *rod.Page, js string) (int, error) {
	res, err := page.Eval(js)
	if err != nil {
		return 0, err
	}
	return res.Value.Int(), nil
}

func runSession(base, chromeBin string) (map[string]any, error) {
	counts := map[string]any{}
	browser, err := newBrowser(chromeBin)
	if err != nil {
		return nil, err
	}
	defer func() { _ = browser.Close() }()

	page, err := browser.Page(proto.TargetCreateTarget{URL: base + "/page_a.html"})
	if err != nil {
		return nil, fmt.Errorf("open A: %w", err)
	}
	if err := page.WaitLoad(); err != nil {
		return nil, fmt.Errorf("load A: %w", err)
	}
	if err := setViewport(page); err != nil {
		return nil, fmt.Errorf("viewport A: %w", err)
	}
	counts["title_a"], err = evalStr(page, evalTitle)
	if err != nil {
		return nil, fmt.Errorf("title A: %w", err)
	}
	counts["cards_a"], err = evalInt(page, evalCardCount)
	if err != nil {
		return nil, fmt.Errorf("card count: %w", err)
	}
	var cards []Card
	if res, err := page.Eval(evalExtract); err != nil {
		return nil, fmt.Errorf("extract: %w", err)
	} else if err := res.Value.Unmarshal(&cards); err != nil {
		return nil, fmt.Errorf("unmarshal extract: %w", err)
	}
	raw, err := json.Marshal(cards)
	if err != nil {
		return nil, fmt.Errorf("marshal extract: %w", err)
	}
	if err := os.WriteFile(os.Getenv("LADDER_EXTRACT_OUT"), raw, 0o644); err != nil {
		return nil, fmt.Errorf("write extract: %w", err)
	}
	q, err := page.Element("#q")
	if err != nil {
		return nil, fmt.Errorf("find #q: %w", err)
	}
	if err := q.Input("widget"); err != nil {
		return nil, fmt.Errorf("fill: %w", err)
	}
	search, err := page.Element("#search")
	if err != nil {
		return nil, fmt.Errorf("find #search: %w", err)
	}
	if err := search.Click(proto.InputMouseButtonLeft, 1); err != nil {
		return nil, fmt.Errorf("click: %w", err)
	}
	counts["visible"], err = evalInt(page, evalVisible)
	if err != nil {
		return nil, fmt.Errorf("visible: %w", err)
	}
	shot, err := page.Screenshot(false, nil)
	if err != nil {
		return nil, fmt.Errorf("screenshot: %w", err)
	}
	if err := os.WriteFile(os.Getenv("LADDER_SHOT_OUT"), shot, 0o644); err != nil {
		return nil, fmt.Errorf("write shot: %w", err)
	}

	// New tab on the same browser.
	page2, err := browser.Page(proto.TargetCreateTarget{URL: base + "/page_b.html"})
	if err != nil {
		return nil, fmt.Errorf("open B: %w", err)
	}
	if err := page2.WaitLoad(); err != nil {
		return nil, fmt.Errorf("load B: %w", err)
	}
	if err := setViewport(page2); err != nil {
		return nil, fmt.Errorf("viewport B: %w", err)
	}
	counts["title_b"], err = evalStr(page2, evalTitle)
	if err != nil {
		return nil, fmt.Errorf("title B: %w", err)
	}
	counts["rows_b"], err = evalInt(page2, evalRows)
	if err != nil {
		return nil, fmt.Errorf("rows B: %w", err)
	}
	return counts, nil
}

func runEval(base, chromeBin string, n int) ([]float64, error) {
	browser, err := newBrowser(chromeBin)
	if err != nil {
		return nil, err
	}
	defer func() { _ = browser.Close() }()

	page, err := browser.Page(proto.TargetCreateTarget{URL: base + "/page_a.html"})
	if err != nil {
		return nil, fmt.Errorf("open A: %w", err)
	}
	if err := page.WaitLoad(); err != nil {
		return nil, fmt.Errorf("load A: %w", err)
	}
	lat := make([]float64, 0, n)
	for i := 0; i < n; i++ {
		t0 := time.Now()
		if _, err := page.Eval(evalTitle); err != nil {
			return nil, fmt.Errorf("eval %d: %w", i, err)
		}
		lat = append(lat, float64(time.Since(t0).Nanoseconds())/1e6)
	}
	return lat, nil
}

func runCold(chromeBin string) error {
	browser, err := newBrowser(chromeBin)
	if err != nil {
		return err
	}
	defer func() { _ = browser.Close() }()

	page, err := browser.Page(proto.TargetCreateTarget{})
	if err != nil {
		return fmt.Errorf("open page: %w", err)
	}
	if _, err := page.Eval(evalOne); err != nil {
		return fmt.Errorf("eval: %w", err)
	}
	return nil
}

// evalNullable evaluates js and decodes a possibly-null string.
func evalNullable(page *rod.Page, js string) (*string, error) {
	res, err := page.Eval(js)
	if err != nil {
		return nil, err
	}
	var s *string
	if err := res.Value.Unmarshal(&s); err != nil {
		return nil, err
	}
	return s, nil
}

func runRealworld(chromeBin string) (map[string]any, error) {
	browser, err := newBrowser(chromeBin)
	if err != nil {
		return nil, err
	}
	defer func() { _ = browser.Close() }()
	page, err := browser.Page(proto.TargetCreateTarget{})
	if err != nil {
		return nil, fmt.Errorf("open page: %w", err)
	}
	ops := map[string]any{}
	t := time.Now()
	if err := page.Navigate("https://example.com"); err != nil {
		return nil, fmt.Errorf("goto: %w", err)
	}
	if err := page.WaitLoad(); err != nil {
		return nil, fmt.Errorf("load: %w", err)
	}
	ops["goto_ms"] = float64(time.Since(t).Nanoseconds()) / 1e6
	t = time.Now()
	title, err := evalNullable(page, `() => document.title`)
	if err != nil {
		return nil, fmt.Errorf("title: %w", err)
	}
	ops["title_ms"] = float64(time.Since(t).Nanoseconds()) / 1e6
	h1, err := evalNullable(page, `() => document.querySelector('h1')?.textContent ?? null`)
	if err != nil {
		return nil, fmt.Errorf("h1: %w", err)
	}
	para, err := evalNullable(page, `() => document.querySelector('p')?.textContent?.trim().slice(0, 80) ?? null`)
	if err != nil {
		return nil, fmt.Errorf("para: %w", err)
	}
	return map[string]any{"title": title, "h1": h1, "para": para, "ops": ops}, nil
}

func runBrowse(chromeBin string) (map[string]any, error) {
	browser, err := newBrowser(chromeBin)
	if err != nil {
		return nil, err
	}
	defer func() { _ = browser.Close() }()
	page, err := browser.Page(proto.TargetCreateTarget{})
	if err != nil {
		return nil, fmt.Errorf("open page: %w", err)
	}
	if err := setViewport(page); err != nil {
		return nil, fmt.Errorf("viewport: %w", err)
	}
	facts := map[string]any{}
	ops := map[string]any{}
	// awesome list
	t := time.Now()
	if err := page.Navigate("https://github.com/sindresorhus/awesome"); err != nil {
		return nil, fmt.Errorf("goto awesome: %w", err)
	}
	if err := page.WaitLoad(); err != nil {
		return nil, fmt.Errorf("load awesome: %w", err)
	}
	ops["awesome_goto_ms"] = float64(time.Since(t).Nanoseconds()) / 1e6
	if facts["awesome_title"], err = evalNullable(page, `() => document.title ?? null`); err != nil {
		return nil, fmt.Errorf("awesome title: %w", err)
	}
	if facts["awesome_links"], err = evalInt(page, `() => document.querySelectorAll('a').length`); err != nil {
		return nil, fmt.Errorf("awesome links: %w", err)
	}
	scrollYs := make([]float64, 0, 5)
	for i := 0; i < 5; i++ {
		t := time.Now()
		res, err := page.Eval(`() => { window.scrollBy({ top: 800, behavior: 'instant' }); return window.scrollY; }`)
		if err != nil {
			return nil, fmt.Errorf("scroll %d: %w", i, err)
		}
		scrollYs = append(scrollYs, res.Value.Num())
		ops[fmt.Sprintf("awesome_scroll%d_ms", i)] = float64(time.Since(t).Nanoseconds()) / 1e6
		time.Sleep(300 * time.Millisecond)
	}
	facts["awesome_scroll_ys"] = scrollYs
	var headings []*string
	if res, err := page.Eval(`() => [...document.querySelectorAll('h2')].slice(0, 5).map(h => h.textContent?.trim() ?? null)`); err != nil {
		return nil, fmt.Errorf("headings: %w", err)
	} else if err := res.Value.Unmarshal(&headings); err != nil {
		return nil, fmt.Errorf("unmarshal headings: %w", err)
	}
	facts["awesome_headings"] = headings
	// trending
	t = time.Now()
	if err := page.Navigate("https://github.com/trending"); err != nil {
		return nil, fmt.Errorf("goto trending: %w", err)
	}
	if err := page.WaitLoad(); err != nil {
		return nil, fmt.Errorf("load trending: %w", err)
	}
	ops["trending_goto_ms"] = float64(time.Since(t).Nanoseconds()) / 1e6
	if facts["trending_title"], err = evalNullable(page, `() => document.title ?? null`); err != nil {
		return nil, fmt.Errorf("trending title: %w", err)
	}
	if facts["trending_repos"], err = evalInt(page, `() => document.querySelectorAll('article.Box-row').length`); err != nil {
		return nil, fmt.Errorf("trending repos: %w", err)
	}
	var top3 []string
	if res, err := page.Eval(`() => [...document.querySelectorAll('article.Box-row h2 a')].slice(0, 3).map(a => (a.textContent ?? '').replace(/\s+/g, ''))`); err != nil {
		return nil, fmt.Errorf("top3: %w", err)
	} else if err := res.Value.Unmarshal(&top3); err != nil {
		return nil, fmt.Errorf("unmarshal top3: %w", err)
	}
	facts["trending_top3"] = top3
	t = time.Now()
	link, err := page.Element("article.Box-row h2 a")
	if err != nil {
		return nil, fmt.Errorf("find repo link: %w", err)
	}
	if err := link.Click(proto.InputMouseButtonLeft, 1); err != nil {
		return nil, fmt.Errorf("click: %w", err)
	}
	var newURL *string
	// Look first, then every 250 ms: a click that already waited for its
	// navigation is seen at once, as any driver's would be.
	for i := 0; i < 24; i++ {
		if i > 0 {
			time.Sleep(250 * time.Millisecond)
		}
		info, err := page.Info()
		if err != nil {
			continue
		}
		if !strings.Contains(info.URL, "trending") {
			newURL = &info.URL
			break
		}
	}
	ops["trending_click_ms"] = float64(time.Since(t).Nanoseconds()) / 1e6
	facts["trending_clicked_url"] = newURL
	if facts["trending_clicked_title"], err = evalNullable(page, `() => document.title ?? null`); err != nil {
		return nil, fmt.Errorf("clicked title: %w", err)
	}
	return map[string]any{"facts": facts, "ops": ops}, nil
}

func emit(v any) {
	raw, err := json.Marshal(v)
	if err != nil {
		fmt.Fprintf(os.Stderr, "[gorod] fatal: marshal: %v\n", err)
		os.Exit(1)
	}
	fmt.Println(string(raw))
}

func main() {
	if err := realMain(); err != nil {
		fmt.Fprintf(os.Stderr, "[gorod] fatal: %v\n", err)
		os.Exit(1)
	}
}

func realMain() error {
	mode := "session"
	if len(os.Args) > 1 {
		mode = os.Args[1]
	}
	base := os.Getenv("LADDER_BASE")
	chromeBin := os.Getenv("CHROME_BIN")
	t0 := time.Now()
	wallMs := func() float64 { return float64(time.Since(t0).Nanoseconds()) / 1e6 }

	switch mode {
	case "session":
		counts, err := runSession(base, chromeBin)
		if err != nil {
			return err
		}
		emit(map[string]any{"mode": "session", "wall_ms": wallMs(), "counts": counts})
	case "eval":
		n := 200
		if s := os.Getenv("LADDER_N_EVAL"); s != "" {
			if v, err := strconv.Atoi(s); err == nil {
				n = v
			}
		}
		lat, err := runEval(base, chromeBin, n)
		if err != nil {
			return err
		}
		emit(map[string]any{"mode": "eval", "wall_ms": wallMs()})
		emit(map[string]any{"eval_ms": lat})
	case "cold":
		if err := runCold(chromeBin); err != nil {
			return err
		}
		emit(map[string]any{"mode": "cold", "wall_ms": wallMs()})
	case "realworld":
		rw, err := runRealworld(chromeBin)
		if err != nil {
			return err
		}
		emit(map[string]any{"mode": "realworld", "wall_ms": wallMs(), "realworld": rw})
	case "browse":
		br, err := runBrowse(chromeBin)
		if err != nil {
			return err
		}
		emit(map[string]any{"mode": "browse", "wall_ms": wallMs(), "realworld": br})
	case "timeline":
		tl, err := runTimeline(chromeBin)
		if err != nil {
			return err
		}
		emit(map[string]any{"mode": "timeline", "wall_ms": wallMs(), "timeline": tl})
	default:
		return fmt.Errorf("unknown mode %q", mode)
	}
	return nil
}

// runTimeline: per-request timeline on https://example.com.
// Same Navigate+WaitLoad as runRealworld, wrapped with Network.* and
// Page.loadEventFired event timestamps so the goto window can be split
// into before-the-response, response-to-load and after-load phases.
// All client times are ms since just before page.Navigate; CDP times are
// the events' own monotonic timestamps (s), reported raw.
func runTimeline(chromeBin string) (map[string]any, error) {
	lt0 := time.Now()
	browser, err := newBrowser(chromeBin)
	if err != nil {
		return nil, err
	}
	defer func() { _ = browser.Close() }()
	launchMs := float64(time.Since(lt0).Nanoseconds()) / 1e6

	page, err := browser.Page(proto.TargetCreateTarget{})
	if err != nil {
		return nil, fmt.Errorf("open page: %w", err)
	}
	if err := (proto.NetworkEnable{}).Call(page); err != nil {
		return nil, fmt.Errorf("network.enable: %w", err)
	}
	if err := (proto.PageEnable{}).Call(page); err != nil {
		return nil, fmt.Errorf("page.enable: %w", err)
	}

	var mu sync.Mutex
	reqAt, respAt, loadAt := -1.0, -1.0, -1.0    // client ms since nav start
	reqCDP, respCDP, loadCDP := -1.0, -1.0, -1.0 // CDP monotonic timestamps (s)
	var rt *proto.NetworkResourceTiming
	var t0 time.Time
	msSince := func(t time.Time) float64 { return float64(time.Since(t0).Nanoseconds()) / 1e6 }

	// rod's event loop runs inside the returned wait function; run it in a
	// goroutine so events are consumed while Navigate/WaitLoad proceed.
	wait := page.EachEvent(func(e *proto.NetworkRequestWillBeSent) {
		if e.Request != nil && strings.HasPrefix(e.Request.URL, "https://example.com/") {
			mu.Lock()
			if reqAt < 0 && t0 != (time.Time{}) {
				reqAt = msSince(time.Now())
				reqCDP = float64(e.Timestamp)
			}
			mu.Unlock()
		}
	}, func(e *proto.NetworkResponseReceived) {
		if e.Response != nil && strings.HasPrefix(e.Response.URL, "https://example.com/") {
			mu.Lock()
			if respAt < 0 && t0 != (time.Time{}) {
				respAt = msSince(time.Now())
				respCDP = float64(e.Timestamp)
				rt = e.Response.Timing
			}
			mu.Unlock()
		}
	}, func(e *proto.PageLoadEventFired) {
		mu.Lock()
		if loadAt < 0 && t0 != (time.Time{}) {
			loadAt = msSince(time.Now())
			loadCDP = float64(e.Timestamp)
		}
		mu.Unlock()
	})
	go wait()

	t0 = time.Now()
	if err := page.Navigate("https://example.com"); err != nil {
		return nil, fmt.Errorf("goto: %w", err)
	}
	navReplyMs := msSince(time.Now())
	if err := page.WaitLoad(); err != nil {
		return nil, fmt.Errorf("load: %w", err)
	}
	waitLoadMs := msSince(time.Now())

	te := time.Now()
	title, err := evalNullable(page, evalTitle)
	if err != nil {
		return nil, fmt.Errorf("title: %w", err)
	}
	evalMs := float64(time.Since(te).Nanoseconds()) / 1e6

	mu.Lock()
	defer mu.Unlock()
	out := map[string]any{
		"launch_ms":    launchMs,
		"nav_reply_ms": navReplyMs,
		"goto_ms":      waitLoadMs,
		"eval_ms":      evalMs,
		"title":        title,
		"req_ms":       reqAt,
		"resp_ms":      respAt,
		"load_ms":      loadAt,
		"req_cdp_s":    reqCDP,
		"resp_cdp_s":   respCDP,
		"load_cdp_s":   loadCDP,
	}
	if rt != nil {
		out["timing"] = map[string]any{
			"dns_ms":     rt.DNSEnd - rt.DNSStart,
			"connect_ms": rt.ConnectEnd - rt.ConnectStart,
			"ssl_ms":     rt.SslEnd - rt.SslStart,
			"send_ms":    rt.SendEnd - rt.SendStart,
			"ttfb_ms":    rt.ReceiveHeadersEnd,
			"proxy_ms":   rt.ProxyEnd - rt.ProxyStart,
		}
	}
	return out, nil
}
