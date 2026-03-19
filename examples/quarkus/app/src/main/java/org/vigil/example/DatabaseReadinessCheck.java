package org.vigil.example;

import jakarta.enterprise.context.ApplicationScoped;

import org.eclipse.microprofile.health.HealthCheck;
import org.eclipse.microprofile.health.HealthCheckResponse;
import org.eclipse.microprofile.health.Readiness;

/**
 * Custom readiness check — maps to vigil's "level: ready" check concept.
 *
 * In a real application this would verify the database connection,
 * message broker, or other external dependencies are available.
 * vigild's "ready" check gates load-balancer traffic via /q/health/ready.
 */
@Readiness
@ApplicationScoped
public class DatabaseReadinessCheck implements HealthCheck {

    @Override
    public HealthCheckResponse call() {
        // Simulate checking an external dependency.
        // Replace with real connectivity check in production.
        boolean dependencyAvailable = true;

        if (dependencyAvailable) {
            return HealthCheckResponse.up("database");
        } else {
            return HealthCheckResponse
                .named("database")
                .down()
                .withData("reason", "connection refused")
                .build();
        }
    }
}
