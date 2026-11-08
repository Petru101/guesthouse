// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * The part of GNOME's session manager that the shell's Power Off and Restart need. Ubuntu's desktop here runs gnome-shell
 * on its own, without gnome-session, and the shell only offers those when org.gnome.SessionManager says they can be done.
 * This answers that it can, has the shell ask the user as gnome-session would, and asks logind once the user confirms.
 * Logging out isn't offered: this desktop has no login screen to go back to. Suspend isn't either, in a VM.
 * Run as the session's user, on the session's bus, before the shell. Usage: nvfwd-session
 */
#include <gio/gio.h>

#define SHELL_NAME "org.gnome.Shell"
#define DIALOG_PATH "/org/gnome/SessionManager/EndSessionDialog"
#define DIALOG_INTERFACE "org.gnome.SessionManager.EndSessionDialog"
/* gnome-session's own values: how long the dialog waits before it goes ahead by itself, and the dialog types. */
#define DIALOG_SECONDS 60
#define DIALOG_SHUTDOWN 1
#define DIALOG_RESTART 2
/* GnomeSession.ActionAvailability. An older shell that wants a boolean reads this as true. */
#define AVAILABLE 3
#define UNAVAILABLE 0

static const char interface_xml[] =
    "<node><interface name='org.gnome.SessionManager'>"
    "<method name='Logout'><arg type='u' direction='in'/></method>"
    "<method name='Shutdown'/><method name='Reboot'/><method name='Suspend'/>"
    "<method name='CanShutdown'><arg type='u' direction='out'/></method>"
    "<method name='CanReboot'><arg type='u' direction='out'/></method>"
    "<method name='CanSuspend'><arg type='u' direction='out'/></method>"
    "<method name='IsInhibited'><arg type='u' direction='in'/><arg type='b' direction='out'/></method>"
    "<property name='SessionIsActive' type='b' access='read'/>"
    "</interface></node>";

static GDBusConnection *system_bus;

/* What the user confirmed, as logind's method for it. */
static void on_confirmed(GDBusConnection *bus, const char *sender, const char *path, const char *interface, const char *signal,
                         GVariant *parameters, gpointer data)
{
    const char *method = NULL;

    if (!g_strcmp0(signal, "ConfirmedShutdown"))
        method = "PowerOff";
    else if (!g_strcmp0(signal, "ConfirmedReboot"))
        method = "Reboot";
    if (!method || !system_bus)
        return;
    g_dbus_connection_call(system_bus, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager",
                           method, g_variant_new("(b)", TRUE), NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
}

/* The shell's own dialog, as gnome-session opens it, with no inhibitors to list. */
static void open_dialog(GDBusConnection *bus, guint type)
{
    GVariantBuilder none;

    g_variant_builder_init(&none, G_VARIANT_TYPE("ao"));
    g_dbus_connection_call(bus, SHELL_NAME, DIALOG_PATH, DIALOG_INTERFACE, "Open",
                           g_variant_new("(uuuao)", type, 0, DIALOG_SECONDS, &none), NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL,
                           NULL, NULL);
}

static void on_call(GDBusConnection *bus, const char *sender, const char *path, const char *interface, const char *method,
                    GVariant *parameters, GDBusMethodInvocation *invocation, gpointer data)
{
    if (!g_strcmp0(method, "CanShutdown") || !g_strcmp0(method, "CanReboot")) {
        g_dbus_method_invocation_return_value(invocation, g_variant_new("(u)", AVAILABLE));
    } else if (!g_strcmp0(method, "CanSuspend")) {
        g_dbus_method_invocation_return_value(invocation, g_variant_new("(u)", UNAVAILABLE));
    } else if (!g_strcmp0(method, "IsInhibited")) {
        g_dbus_method_invocation_return_value(invocation, g_variant_new("(b)", FALSE));
    } else if (!g_strcmp0(method, "Shutdown") || !g_strcmp0(method, "Reboot")) {
        open_dialog(bus, !g_strcmp0(method, "Shutdown") ? DIALOG_SHUTDOWN : DIALOG_RESTART);
        g_dbus_method_invocation_return_value(invocation, NULL);
    } else {
        g_dbus_method_invocation_return_error_literal(invocation, G_DBUS_ERROR, G_DBUS_ERROR_NOT_SUPPORTED,
                                                      "Not on this desktop");
    }
}

static GVariant *on_property(GDBusConnection *bus, const char *sender, const char *path, const char *interface,
                             const char *property, GError **error, gpointer data)
{
    return g_variant_new_boolean(TRUE);
}

static const GDBusInterfaceVTable vtable = { on_call, on_property, NULL, { 0 } };

static void on_bus(GDBusConnection *bus, const char *name, gpointer data)
{
    GDBusNodeInfo *node = g_dbus_node_info_new_for_xml(interface_xml, NULL);

    g_dbus_connection_register_object(bus, "/org/gnome/SessionManager", node->interfaces[0], &vtable, NULL, NULL, NULL);
    g_dbus_connection_signal_subscribe(bus, NULL, DIALOG_INTERFACE, NULL, DIALOG_PATH, NULL, G_DBUS_SIGNAL_FLAGS_NONE,
                                       on_confirmed, NULL, NULL);
    g_dbus_node_info_unref(node);
}

int main(void)
{
    GMainLoop *loop = g_main_loop_new(NULL, FALSE);

    system_bus = g_bus_get_sync(G_BUS_TYPE_SYSTEM, NULL, NULL);
    g_bus_own_name(G_BUS_TYPE_SESSION, "org.gnome.SessionManager", G_BUS_NAME_OWNER_FLAGS_NONE, on_bus, NULL, NULL, NULL,
                   NULL);
    g_main_loop_run(loop);
    return 0;
}
