package id.or.oo.pr.session

data class Bind(val hostPath: String, val guestPath: String) {
    init {
        require(hostPath.startsWith("/")) { "bind host path must be absolute: \"$hostPath\"" }
        require(guestPath.startsWith("/")) { "bind guest path must be absolute: \"$guestPath\"" }
        require(':' !in hostPath) { "bind host path must not contain ':': \"$hostPath\"" }
    }
}
