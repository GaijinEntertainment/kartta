package main

type closer interface {
	close()
}

var first = second
var second = first

func cycle() {
	first.Set()
}

func shared() {}

func caller01() { shared() }
func caller02() { shared() }
func caller03() { shared() }
func caller04() { shared() }
func caller05() { shared() }
func caller06() { shared() }
func caller07() { shared() }
func caller08() { shared() }
func caller09() { shared() }
func caller10() { shared() }
func caller11() { shared() }
func caller12() { shared() }
func caller13() { shared() }
func caller14() { shared() }
func caller15() { shared() }
func caller16() { shared() }
func caller17() { shared() }
func caller18() { shared() }
func caller19() { shared() }
func caller20() { shared() }
func caller21() { shared() }
func caller22() { shared() }
func caller23() { shared() }
func caller24() { shared() }
func caller25() { shared() }
func caller26() { shared() }
func caller27() { shared() }
func caller28() { shared() }
func caller29() { shared() }
func caller30() { shared() }
func caller31() { shared() }
