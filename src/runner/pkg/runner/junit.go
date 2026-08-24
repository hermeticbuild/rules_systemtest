package runner

import (
	"encoding/xml"
	"fmt"
	"os"
	"time"
)

// junitTestsuite/junitTestcase encode the minimal JUnit XML shape CI systems
// consume: one <testsuite> wrapping exactly one <testcase>, named after the
// systemtest label (CONTRACT.md step 9).
type junitTestsuite struct {
	XMLName   xml.Name        `xml:"testsuite"`
	Name      string          `xml:"name,attr"`
	Tests     int             `xml:"tests,attr"`
	Failures  int             `xml:"failures,attr"`
	Time      float64         `xml:"time,attr"`
	TestCases []junitTestcase `xml:"testcase"`
}

type junitTestcase struct {
	Name      string        `xml:"name,attr"`
	Classname string        `xml:"classname,attr"`
	Time      float64       `xml:"time,attr"`
	Failure   *junitFailure `xml:"failure,omitempty"`
}

type junitFailure struct {
	Message string `xml:"message,attr"`
	Text    string `xml:",chardata"`
}

// writeJUnitReport writes a minimal one-testcase JUnit report to path (a
// no-op if path is empty, i.e. $XML_OUTPUT_FILE was unset). The testcase is
// marked failed iff exitCode != 0.
func writeJUnitReport(path, label string, exitCode int, duration time.Duration) error {
	if path == "" {
		return nil
	}

	tc := junitTestcase{Name: label, Classname: label, Time: duration.Seconds()}
	failures := 0
	if exitCode != 0 {
		failures = 1
		tc.Failure = &junitFailure{
			Message: fmt.Sprintf("exited with code %d", exitCode),
			Text:    fmt.Sprintf("test binary exited with code %d", exitCode),
		}
	}
	suite := junitTestsuite{
		Name:      label,
		Tests:     1,
		Failures:  failures,
		Time:      duration.Seconds(),
		TestCases: []junitTestcase{tc},
	}

	out, err := xml.MarshalIndent(suite, "", "  ")
	if err != nil {
		return fmt.Errorf("marshaling junit report: %w", err)
	}
	content := append([]byte(xml.Header), out...)
	if err := os.WriteFile(path, content, 0o644); err != nil {
		return fmt.Errorf("writing junit report to %s: %w", path, err)
	}
	return nil
}
